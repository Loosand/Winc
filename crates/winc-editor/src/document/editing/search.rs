//! 文件内查找与替换：纯文本、正则和跨行查找，替换模板展开，以及按字节判断整词。
//!
//! 搜索按时间片分段进行，每次 `update` 最多耗时 `SLICE`，长文件不会阻塞界面；结果按位置排序，
//! 匹配数达到 [`MAX_MATCHES`] 后停止搜索并标记结果不完整。`Matcher` 编译一次查询后供多次查找
//! 使用：`Pattern` 在 `regex` 与 `fancy-regex` 之间选择实现，回溯过多时返回 `GaveUp`。

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use memchr::memmem::Finder;
use regex::{Regex, RegexBuilder};

use winc_core::{Change, EditKind, Lines, Selection, Transaction};
use winc_i18n::t;

use crate::DocumentId;
use crate::document::Document;

/// 最多记录的匹配数（处）。
///
/// 达到该数量时计数显示为「10000+」，跳转只在这些匹配之间循环。
pub const MAX_MATCHES: usize = 10_000;
/// 单次 `update` 的搜索时间上限（毫秒）。
///
/// 经验值：限制一次调用阻塞界面的时间。
const SLICE: Duration = Duration::from_millis(8);
/// 检查一次时钟之间处理的行数（行）。
///
/// 避免每行都读取时钟。
const CLOCK_INTERVAL: usize = 256;
/// 检查一次时钟之间处理的字节数（字节，256 KiB），与 `CLOCK_INTERVAL` 先到者为准。
///
/// 只按行数计时，大文件中每行几 MB 时 256 行就是数百 MB，一次调用会阻塞界面数秒。经验值。
const CLOCK_BYTES: usize = 256 << 10;

/// 查找选项：大小写敏感、整词匹配和正则表达式模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// 是否区分大小写。
    pub case_sensitive: bool,
    /// 是否要求全字匹配：匹配的两端不能紧挨着字母、数字或下划线。
    pub whole_word: bool,
    /// 是否按正则匹配。
    pub regex: bool,
}

pub(crate) type Options = SearchOptions;

/// 查找条渲染与测试所需的只读状态快照。
#[derive(Debug, Clone, Copy)]
pub struct SearchStatus<'a> {
    /// 当前查找内容。
    pub query: &'a str,
    /// 当前匹配在结果中的下标（从 0 起）。
    pub current: Option<usize>,
    /// 已找到的匹配数。
    pub matches: usize,
    /// 结果是否因达到 [`MAX_MATCHES`] 或正则回溯超限而不完整。
    pub incomplete: bool,
    /// 是否仍在分片搜索中。
    pub searching: bool,
    /// 查询无法编译或大文件跨行受限时的错误说明。
    pub error: Option<&'a str>,
    /// 当前启用的查找选项。
    pub options: SearchOptions,
}

/// 不依赖文档的查找：在给定的文字里按和 [`Search`] 相同的条件找，语义完全相同。
#[derive(Debug)]
pub struct TextSearch(Matcher);

impl TextSearch {
    /// 编译查询，各条件含义同 [`SearchOptions`]。
    ///
    /// # Errors
    ///
    /// 正则表达式有误时返回错误说明。
    pub fn new(
        query: &str,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
    ) -> Result<Self, String> {
        let options = SearchOptions {
            case_sensitive,
            whole_word,
            regex,
        };
        Matcher::new(query, options).map(TextSearch)
    }

    /// 返回 `text` 里所有不重叠匹配的字节范围，从前到后。
    pub fn find_all(&self, text: &str) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        let _ = self.0.find_all(text, |start, end| {
            found.push((start, end));
            ControlFlow::Continue(())
        });
        found
    }
}

/// 一个文档上的查找状态，包括条件、匹配结果和替换串。
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// 查找的内容；为空时没有匹配。
    query: String,
    /// 替换成的文字。
    ///
    /// 正则模式下 `$1`、`${name}` 引用分组，`\n` 换成文档的换行符，`\t` 换成制表符。
    replacement: String,
    /// 当前查找选项。
    options: SearchOptions,
    /// 当前匹配在 `matches` 中的下标（从 0 起）。
    current: Option<usize>,
    /// 匹配的位置范围（`char` 偏移，左闭右开），按位置排序、互不重叠。
    matches: Vec<(usize, usize)>,
    /// 有匹配的行号，升序、不重复。
    lines: Vec<usize>,
    /// 结果所属的文档及其版本；与本次相同时不重新搜索。
    key: Option<Key>,
    /// 编译 `matcher` 时的查询和选项；与当前条件相同时不必重新编译。
    compiled: Option<(String, SearchOptions)>,
    /// 按 `compiled` 的条件编译好的匹配器。
    ///
    /// 查询为空或无法编译时为 `None`。
    matcher: Option<Matcher>,
    /// 查询无法编译或不被支持时的错误说明。
    error: Option<String>,
    /// 搜索未完成时的断点，下次 `update` 从这里继续；已完成时为 `None`。
    resume: Option<Resume>,
    /// 已搜索的行里，最早一处因正则回溯过多而放弃查找的行号；没有时为 `None`。
    gave_up: Option<usize>,
}

/// 搜索结果所属的文档、版本，以及差异快照的一侧。
///
/// 查询和选项另记在 `Search::compiled` 中，比较时不必复制查询。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    doc: DocumentId,
    revision: u64,
    /// 在差异标签的哪一侧快照中搜索（`LEFT` 或 `RIGHT`）；搜索文档自身的文本时为 `None`。
    snapshot_side: Option<usize>,
}

/// 搜索断点：下次从第 `line` 行（从 0 起）继续，`pos` 是该行行首的位置（字符偏移）。
#[derive(Debug, Clone, Copy)]
struct Resume {
    line: usize,
    pos: usize,
}

impl Search {
    /// 以给定的查找内容构造查找状态（默认不区分大小写、非整词、非正则）。
    pub fn with_query(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

    /// 返回供界面渲染使用的查找状态快照。
    pub fn status(&self) -> SearchStatus<'_> {
        SearchStatus {
            query: &self.query,
            current: self.current,
            matches: self.matches.len(),
            incomplete: self.is_truncated() || self.gave_up(),
            searching: !self.is_complete(),
            error: self.error.as_deref(),
            options: self.options,
        }
    }

    /// 是否设置了非空的查找内容。
    pub fn has_query(&self) -> bool {
        !self.query.is_empty()
    }

    /// 当前是否正处于正则模式且查找内容恰好等于 `pattern`（供 Vim 高亮同步判断）。
    pub fn matches_regex_query(&self, pattern: &str) -> bool {
        self.options.regex && self.query == pattern
    }

    /// 设置新的查找内容；内容变化时作废已编译匹配器，清空查询时立即清除历史结果。
    pub fn set_query(&mut self, query: impl Into<String>) {
        let next = query.into();
        if self.query == next {
            return;
        }
        self.query = next;
        self.invalidate_query();
    }

    /// 清空查找内容并重置已有匹配结果。
    pub fn clear_query(&mut self) {
        if self.query.is_empty() && self.matches.is_empty() && self.error.is_none() {
            return;
        }
        self.query.clear();
        self.invalidate_query();
    }

    /// 更新替换文字。
    pub fn set_replacement(&mut self, replacement: impl Into<String>) {
        let next = replacement.into();
        if self.replacement != next {
            self.replacement = next;
        }
    }

    /// 切换「区分大小写」选项并作废当前匹配结果。
    pub fn toggle_case_sensitive(&mut self) {
        self.options.case_sensitive = !self.options.case_sensitive;
        self.invalidate_query();
    }

    /// 切换「全字匹配」选项并作废当前匹配结果。
    pub fn toggle_whole_word(&mut self) {
        self.options.whole_word = !self.options.whole_word;
        self.invalidate_query();
    }

    /// 切换「正则表达式」选项并作废当前匹配结果。
    pub fn toggle_regex(&mut self) {
        self.options.regex = !self.options.regex;
        self.invalidate_query();
    }

    /// 一次性设置全部查找选项；任一选项变化时作废已编译匹配器。
    pub fn set_options(&mut self, options: SearchOptions) {
        if self.options == options {
            return;
        }
        self.options = options;
        self.invalidate_query();
    }

    /// 原子配置一次正则查找（供 Vim 搜索与查找条同步使用）。
    pub fn configure_regex(&mut self, pattern: impl Into<String>, case_sensitive: bool) {
        let pattern = pattern.into();
        let options = SearchOptions {
            case_sensitive,
            whole_word: false,
            regex: true,
        };
        if self.query == pattern && self.options == options {
            return;
        }
        self.query = pattern;
        self.options = options;
        self.invalidate_query();
    }

    /// 切换或关闭标签页时清除当前定位的匹配序号与未完成断点。
    pub fn reset_position(&mut self) {
        self.current = None;
        self.resume = None;
    }

    /// 查询或选项改动后作废已编译匹配器；查询为空时立即清空结果。
    fn invalidate_query(&mut self) {
        self.compiled = None;
        if self.query.is_empty() {
            self.matcher = None;
            self.error = None;
            self.restart();
        }
    }

    /// 返回结果所属的文档标识；还没搜过时为 `None`。
    pub fn document(&self) -> Option<DocumentId> {
        self.key.as_ref().map(|k| k.doc)
    }

    /// 返回当前结果所属的差异快照一侧；结果不属于 `doc` 的当前版本，或不是快照的结果时
    /// 返回 `None`。
    pub fn snapshot_side(&self, doc: &Document) -> Option<usize> {
        self.key
            .filter(|k| k.doc == doc.id && k.revision == doc.revision)?
            .snapshot_side
    }

    /// 返回全部匹配的位置范围（`char` 偏移），按起点排序、互不重叠。
    pub fn matches(&self) -> &[(usize, usize)] {
        &self.matches
    }

    /// 返回有匹配的行号（从 0 起），升序且不重复。
    pub fn lines(&self) -> &[usize] {
        &self.lines
    }

    /// 返回与位置范围 `from` 到 `to`（字符偏移，左闭右开）有交集的匹配。
    pub fn matches_in(&self, from: usize, to: usize) -> &[(usize, usize)] {
        let start = self.matches.partition_point(|&(_, end)| end <= from);
        let end = start + self.matches[start..].partition_point(|&(begin, _)| begin < to);
        &self.matches[start..end]
    }

    /// 返回当前匹配的位置范围；没有当前匹配时返回 `None`。
    pub fn current_match(&self) -> Option<(usize, usize)> {
        self.current.and_then(|i| self.matches.get(i).copied())
    }

    /// 是否因为达到 [`MAX_MATCHES`] 而截断，此时结果不完整。
    pub fn is_truncated(&self) -> bool {
        self.matches.len() >= MAX_MATCHES
    }

    /// 是否有行因正则回溯过多而放弃查找，此时结果可能不全：计数是下限，不能全部替换。
    pub fn gave_up(&self) -> bool {
        self.gave_up.is_some()
    }

    /// 是否已搜索到文档末尾，即没有因时间片用尽而暂停。
    pub fn is_complete(&self) -> bool {
        self.resume.is_none()
    }

    /// 返回已搜索范围的行数上界：行号（从 0 起）小于该值的行都已搜索过。
    ///
    /// 搜索完成时为 `usize::MAX`；因达到 [`MAX_MATCHES`] 而停止时，为最后一个匹配所在行的下一行，
    /// 之后的行不算已搜索。
    pub fn searched_lines(&self) -> usize {
        match self.resume {
            Some(resume) => resume.line,
            None if self.is_truncated() => self.lines.last().map_or(0, |&l| l + 1),
            None => usize::MAX,
        }
    }

    /// 是否由 `doc` 的当前内容搜索得到：文档标识与 `revision` 都相同，且不是差异快照的结果。
    ///
    /// 文档内容修改后、下一次 `update` 之前为 `false`。
    pub fn is_for(&self, doc: &Document) -> bool {
        self.key.as_ref().is_some_and(|k| {
            k.doc == doc.id && k.revision == doc.revision && k.snapshot_side.is_none()
        })
    }

    /// 返回查询的错误说明（正则语法错误，或大文件上的跨行查找）；没有错误时返回 `None`。
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 查询或选项变化时重新编译匹配器、作废已有结果，下次 [`Self::update`] 从头搜索。
    ///
    /// 条件未变化时直接返回。替换只需要匹配器、不需要搜索结果，因此只调用这一步。
    fn ensure_matcher(&mut self) {
        let options = self.options;
        if self
            .compiled
            .as_ref()
            .is_some_and(|(query, compiled)| *query == self.query && *compiled == options)
        {
            return;
        }

        self.matcher = None;
        self.error = None;
        if !self.query.is_empty() {
            match Matcher::new(&self.query, options) {
                Ok(matcher) => self.matcher = Some(matcher),
                Err(error) => self.error = Some(error),
            }
        }
        self.compiled = Some((self.query.clone(), options));
        self.restart();
    }

    /// 文档改动后能否保留改动之上的结果：跨行匹配可能从上方伸进改动的行，不能保留。
    fn resume_kept(&self) -> bool {
        self.matcher.as_ref().is_some_and(|m| !m.is_multiline())
    }

    /// 前 `kept` 行没变：保留这些行里的匹配，下次 [`Self::update`] 从第 `kept` 行（或上次尚未
    /// 搜到的更早位置）继续搜索。
    ///
    /// `kept` 为 0 时同 [`Self::restart`]。
    fn restart_from(&mut self, kept: usize, doc: &Document) {
        let line = kept.min(self.searched_lines());
        let text = doc.text();
        if line == 0 || line >= text.line_count() {
            self.restart();
            return;
        }
        let pos = text.line_start(line);
        let matches = self.matches.partition_point(|&(start, _)| start < pos);
        self.matches.truncate(matches);
        let lines = self.lines.partition_point(|&l| l < line);
        self.lines.truncate(lines);
        self.gave_up = self.gave_up.filter(|&l| l < line);
        self.current = None;
        // 留下的匹配已达到上限：与搜到上限时相同，不再继续搜索。
        self.resume = (matches < MAX_MATCHES).then_some(Resume { line, pos });
    }

    /// 清空结果，下次 [`Self::update`] 从第 0 行开始搜索。
    fn restart(&mut self) {
        self.matches.clear();
        self.lines.clear();
        self.gave_up = None;
        self.current = None;
        self.resume = self.matcher.is_some().then_some(Resume { line: 0, pos: 0 });
    }

    /// 按当前条件更新匹配结果，单次调用最多耗时 `SLICE`。
    ///
    /// 查询或选项变化时从头搜索；文档改动只丢弃改动所在行起的结果，从那一行继续搜索（见
    /// [`Document::unchanged_lines_since`]）；上次未搜完时从断点继续一个时间片。跨行查找（正则
    /// 含 `\n` 等）不分片，一次搜完，文档改动后从头搜索。
    pub fn update(&mut self, doc: &Document) {
        // 仅文档内容变化时，已编译的匹配器仍然有效，不必重新编译。
        self.ensure_matcher();
        let key = Key {
            doc: doc.id,
            revision: doc.revision,
            snapshot_side: None,
        };
        if self.key != Some(key) {
            let kept = match self.key {
                Some(old)
                    if old.doc == doc.id && old.snapshot_side.is_none() && self.resume_kept() =>
                {
                    doc.unchanged_lines_since(old.revision)
                }
                _ => 0,
            };
            self.key = Some(key);
            self.restart_from(kept, doc);
        }
        self.scan(doc.text(), !doc.text().is_large());
    }

    /// 在差异标签 `doc` 第 `side` 侧的快照文本 `text` 中推进查找，单次调用最多耗时 `SLICE`。
    ///
    /// 快照不可修改，结果按（标签，版本，侧）区分，任一项变化时从头搜索。
    pub fn update_snapshot(&mut self, doc: &Document, side: usize, text: &ropey::Rope) {
        self.ensure_matcher();
        let key = Key {
            doc: doc.id,
            revision: doc.revision,
            snapshot_side: Some(side),
        };
        if self.key != Some(key) {
            self.key = Some(key);
            self.restart();
        }
        self.scan(text, true);
    }

    /// 从上次的断点起在 `text` 中继续搜索一个时间片；`multiline` 表示能否取得整篇文本，
    /// 不能时不支持跨行查找。
    fn scan(&mut self, text: &impl Lines, multiline: bool) {
        let (Some(from), Some(matcher)) = (self.resume.take(), &self.matcher) else {
            return;
        };
        let (matches, lines, gave_up) = (&mut self.matches, &mut self.lines, &mut self.gave_up);

        // 跨行查找需要整篇文本，一次搜完。大文件按需从磁盘读取，无法取得整篇文本，因此不支持。
        if matcher.is_multiline() {
            if !multiline {
                self.error = Some(t!("editor-find-large-multiline").to_owned());
                return;
            }
            let (joined, starts) = joined(text);
            matcher.scan_joined(&joined, &starts, |m| {
                matches.push(m.range);
                for line in m.line..=m.last {
                    if lines.last().is_none_or(|&l| l < line) {
                        lines.push(line);
                    }
                }
                if matches.len() >= MAX_MATCHES {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            });
            if matcher.take_gave_up() {
                *gave_up = Some(0);
            }
            return;
        }

        let started = Instant::now();
        let (mut count, mut bytes, mut full, mut paused_at) = (0, 0, false, 0);

        let stopped = text.visit_lines(from.line, usize::MAX, |index, start, line| {
            count += 1;

            if count % CLOCK_INTERVAL == 0 || bytes >= CLOCK_BYTES {
                bytes = 0;
                if started.elapsed() >= SLICE {
                    paused_at = start;
                    return ControlFlow::Break(());
                }
            }
            bytes += line.len();

            let flow = matcher.scan(index, start, line, matches, lines, MAX_MATCHES);
            if matcher.take_gave_up() {
                gave_up.get_or_insert(index);
            }
            full = flow.is_break();
            flow
        });

        self.resume = match stopped {
            Some(line) if !full => Some(Resume {
                line,
                pos: paused_at,
            }),
            _ => None,
        };
    }

    /// 返回起点不早于 `pos` 的第一个匹配，并设为当前匹配；`backwards` 时返回起点早于 `pos` 的
    /// 最后一个匹配。
    ///
    /// 到达一端后绕回另一端；没有任何匹配时返回 `None`。搜索尚未完成时只返回确定是「下一个」的
    /// 匹配，不会提前绕回另一端。
    pub fn next_from(&mut self, pos: usize, backwards: bool) -> Option<(usize, usize)> {
        // `matches` 按起点排序：`i` 是第一个起点不早于 `pos` 的匹配。
        let i = self.matches.partition_point(|&(start, _)| start < pos);
        let found = if backwards {
            // 断点早于 `pos` 时，`pos` 之前可能还有尚未找到的匹配，无法确定「上一个」。
            let scanned = self.resume.is_none_or(|r| r.pos >= pos);
            scanned.then(|| i.checked_sub(1)).flatten()
        } else {
            (i < self.matches.len()).then_some(i)
        };

        let index = match found {
            Some(index) => index,
            None if self.is_complete() && !self.matches.is_empty() => {
                if backwards {
                    self.matches.len() - 1
                } else {
                    0
                }
            }
            None => return None,
        };

        self.current = Some(index);
        Some(self.matches[index])
    }

    /// 返回 `ranges`（按位置排序、互不重叠）中仍是匹配的位置及各自的替换文字，正则模式下已
    /// 展开分组引用和转义。
    ///
    /// 在对应的行上重新匹配，不依赖已记录的 `matches`，因此超过 [`MAX_MATCHES`] 而尚未搜到的
    /// 位置也能判断，搜索之后文档被修改也不会替换到错误位置。
    fn edits_for(
        &self,
        doc: &Document,
        ranges: &[(usize, usize)],
    ) -> Vec<((usize, usize), String)> {
        let Some(matcher) = &self.matcher else {
            return Vec::new();
        };
        let regex = matcher.template_regex();
        let template = regex.map(|_| unescape(&self.replacement, doc.line_ending.as_str()));
        let text = doc.text();
        let mut edits = Vec::new();

        // 跨行查找：在整篇文本上重新匹配，只替换 `ranges` 中仍恰好是匹配的位置；
        // 大文件不支持跨行查找，返回空结果。
        if matcher.is_multiline() {
            if text.rope().is_none() {
                return edits;
            }
            let (joined, starts) = joined(text);
            let mut found = Vec::new();
            matcher.scan_joined(&joined, &starts, |m| {
                found.push((m.range, m.bytes));
                ControlFlow::Continue(())
            });
            for &range in ranges {
                let Ok(i) = found.binary_search_by(|(r, _)| r.cmp(&range)) else {
                    continue;
                };
                let (s, e) = found[i].1;
                let inserted = match (regex, &template) {
                    (Some(regex), Some(template)) => {
                        let Some(replaced) = regex.expand_at(&joined, (s, e), template) else {
                            continue;
                        };
                        replaced
                    }
                    _ => self.replacement.clone(),
                };
                edits.push((range, inserted));
            }
            return edits;
        }
        // 一行内的匹配：（在文档中的位置范围，在该行内的字节范围）。
        let mut found: Vec<((usize, usize), (usize, usize))> = Vec::new();
        let mut rest = ranges;

        while let Some(&(first, _)) = rest.first() {
            let line = text.line_of(first);
            let line_start = text.line_start(line);
            let line_end = line_start + text.line_len(line);
            let count = rest
                .iter()
                .take_while(|r| r.0 < line_end.max(first + 1))
                .count();
            let (here, next) = rest.split_at(count);
            rest = next;

            let content = text.line_content(line).slice().to_string();
            let bytes = content.as_bytes();
            let (mut byte, mut pos) = (0, line_start);
            found.clear();
            let _ = matcher.find_all(&content, |s, e| {
                let start = pos + count_chars(&bytes[byte..s]);
                (byte, pos) = (e, start + count_chars(&bytes[s..e]));
                found.push(((start, pos), (s, e)));
                ControlFlow::Continue(())
            });

            for &range in here {
                let Ok(i) = found.binary_search_by(|(r, _)| r.cmp(&range)) else {
                    continue;
                };
                let inserted = match (regex, &template) {
                    (Some(regex), Some(template)) => {
                        let Some(replaced) = regex.expand_at(&content, found[i].1, template) else {
                            continue;
                        };
                        replaced
                    }
                    _ => self.replacement.clone(),
                };
                edits.push((range, inserted));
            }
        }

        edits
    }

    /// 替换与选区完全重合的匹配，返回是否发生了替换；替换后光标位于新文字之后。
    /// 选区不与任何匹配完全重合时不替换并返回 `Ok(false)`，调用方可以转而选中下一个匹配。
    ///
    /// # Errors
    ///
    /// 文档只读时返回只读原因；被替换的文字超过复制上限而无法取出时返回错误说明。
    pub fn replace_current(&mut self, doc: &mut Document) -> Result<bool, &'static str> {
        if let Some(reason) = doc.readonly_reason() {
            return Err(reason);
        }

        self.ensure_matcher();
        let range = (doc.selection.from(), doc.selection.to());
        let edits = self.edits_for(doc, &[range]);
        let Some((_, inserted)) = edits.first() else {
            return Ok(false);
        };

        let caret = range.0 + inserted.chars().count();
        let transaction = replace_transaction(doc, &edits, Selection::point(caret))?;
        doc.apply(transaction);
        Ok(true)
    }

    /// 替换 `ranges`（按位置排序、互不重叠）中仍与匹配完全一致的位置，作为一次可撤销的修改；
    /// 已不是匹配的位置会被跳过。
    ///
    /// 返回替换的处数。
    ///
    /// # Errors
    ///
    /// 文档只读时返回只读原因；被替换的文字超过复制上限而无法取出时返回错误说明。
    pub fn replace_ranges(
        &mut self,
        doc: &mut Document,
        ranges: &[(usize, usize)],
    ) -> Result<usize, &'static str> {
        if let Some(reason) = doc.readonly_reason() {
            return Err(reason);
        }

        self.ensure_matcher();
        let edits = self.edits_for(doc, ranges);
        let Some(&((first, _), _)) = edits.first() else {
            return Ok(0);
        };

        let transaction = replace_transaction(doc, &edits, Selection::point(first))?;
        doc.apply(transaction);
        Ok(edits.len())
    }

    /// 替换全部匹配，作为一次可撤销的修改。
    ///
    /// 返回替换的处数。
    ///
    /// # Errors
    ///
    /// 文档只读、搜索尚未完成、匹配数达到 [`MAX_MATCHES`]，或被替换的文字超过复制上限而无法取出时，
    /// 返回错误说明。
    pub fn replace_all(&mut self, doc: &mut Document) -> Result<usize, &'static str> {
        if let Some(reason) = doc.readonly_reason() {
            return Err(reason);
        }

        self.update(doc);

        if !self.is_complete() {
            return Err(t!("editor-replace-all-pending"));
        }

        if self.is_truncated() {
            return Err(t!("editor-replace-all-too-many"));
        }

        // 放弃查找的行里可能还有匹配，只替换找到的会留下一部分没换。
        if self.gave_up() {
            return Err(t!("editor-replace-all-gave-up"));
        }

        let ranges = self.matches.clone();
        self.replace_ranges(doc, &ranges)
    }
}

/// 构造替换 `edits`（按位置排序、互不重叠）的事务；变更从后往前排列，使前面的位置不受影响。
fn replace_transaction(
    doc: &Document,
    edits: &[((usize, usize), String)],
    after: Selection,
) -> Result<Transaction, &'static str> {
    let changes = edits
        .iter()
        .rev()
        .map(|((start, end), inserted)| {
            Some(Change {
                start: *start,
                removed: doc.text_range(*start, *end)?,
                inserted: inserted.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(t!("editor-replace-too-long"))?;

    Ok(Transaction::new(
        changes,
        doc.selection,
        after,
        EditKind::Other,
    ))
}

/// 展开替换文字中的转义：`\n` 换成 `line_ending`，`\t` 换成制表符，`\\` 换成单个反斜杠，
/// 其余原样保留。
pub(crate) fn unescape(template: &str, line_ending: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars();

    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('n')) => out.push_str(line_ending),
            ('\\', Some('t')) => out.push('\t'),
            ('\\', Some('\\')) => out.push('\\'),
            _ => {
                out.push(c);
                continue;
            }
        }

        chars.next();
    }

    out
}

/// 已编译的查询，文件内查找与全局搜索共用同一套匹配规则。
#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    kind: Kind,
    whole_word: bool,
    /// 是否为正则模式；为 `true` 时替换文字中的 `$1`、`\n` 需要展开。
    ///
    /// 不区分大小写的纯文本虽然也编译为正则，但其替换文字按字面处理。
    regex: bool,
    /// 是否跨行查找（见 `is_multiline`）：在整篇文本上匹配，不逐行匹配。
    multiline: bool,
    /// 在多行文本块上预先查找候选位置的正则（见 [`Self::block_find`]）。
    ///
    /// `^`、`$` 按行首、行尾匹配，换行识别 `\n`、`\r\n`、`\r`。查询含 `\A`、`\z`（无法放宽），
    /// 或使用 `Kind::Literal`、跨行查找时为 `None`。查询用到环视、反向引用时，这里是
    /// `relaxed` 放宽后的版本；调用方给出必含的文字时（见 [`Self::requiring`]），这里只找这段文字。
    block: Option<Regex>,
    /// 是否有匹配因回溯超出 `fancy-regex` 的上限而放弃：放弃的那一处之后，同一行（跨行查找时为
    /// 整篇文本）不再查找，结果不完整。
    ///
    /// 各副本共用（并行搜索的每个线程一个副本）；由 [`Self::take_gave_up`] 读出并清除。
    gave_up: Arc<AtomicBool>,
}

/// 查询的匹配方式。
#[derive(Debug, Clone)]
enum Kind {
    /// 区分大小写的纯文本，按字节直接查找。
    ///
    /// 放在堆上：`x86_64` 上的查找器包含几种 SIMD 实现，有近 300 字节，比正则大得多。
    Literal(Box<Finder<'static>>),
    /// 正则，以及不区分大小写的纯文本（转义后编译为正则）。
    ///
    /// 大小写按 Unicode 简单大小写折叠处理：Σ、σ、ς 视为同一字母，开尔文符号（U+212A）视为 `k`。
    /// 纯文本与正则的折叠规则相同。
    Regex(Pattern),
}

/// 正则匹配因回溯过多而放弃，见 [`Pattern::find_at`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GaveUp;

/// 一处匹配中各分组的字节范围：第 0 项是整个匹配，没有参与匹配的分组为 `None`。
pub(crate) type Groups = Vec<Option<(usize, usize)>>;

/// 用户查询对应的正则，按语法选择实现。
///
/// `regex` 不支持的写法使用 `fancy-regex` 编译（支持回溯），包括环视（`(?=…)`、`(?<=…)`、`(?!…)`、
/// `(?<!…)`）、反向引用（`\1`）和原子组；其余写法使用 `regex`，其匹配时间与输入长度成线性关系。
#[derive(Debug, Clone)]
pub(crate) enum Pattern {
    /// 由 `regex` 编译的正则。
    Plain(Regex),
    /// 由 `fancy-regex` 编译的正则。
    Fancy(fancy_regex::Regex),
}

impl Pattern {
    /// 编译 `pattern`，优先使用 `regex`，其不支持时回退到 `fancy-regex`。
    ///
    /// # Errors
    ///
    /// 两种实现都无法编译时返回错误说明。
    pub(crate) fn new(
        pattern: &str,
        case_insensitive: bool,
        multi_line: bool,
    ) -> Result<Pattern, String> {
        let error = match RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .multi_line(multi_line)
            .build()
        {
            Ok(regex) => return Ok(Pattern::Plain(regex)),
            Err(error) => error.to_string(),
        };
        match fancy_regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .multi_line(multi_line)
            .build()
        {
            Ok(regex) => Ok(Pattern::Fancy(regex)),
            // 两种实现都编译失败。`regex` 仅因写法不受支持而失败时，报告 `fancy-regex` 的错误
            // （`fancy-regex` 支持环视和反向引用）；否则属于语法错误，报告 `regex` 的错误，
            // 其位置更准确。
            Err(fancy) if error.contains("not supported") => Err(fancy.to_string()),
            Err(_) => Err(error),
        }
    }

    /// 返回从字节偏移 `from` 起的第一个匹配（字节范围）；`from` 之前的文字仍参与环视判断。
    ///
    /// # Errors
    ///
    /// 回溯超过 `fancy-regex` 的上限（默认 100 万次）或回溯栈溢出时返回 [`GaveUp`]：放弃这一处，
    /// 不继续回溯，避免模式导致长时间阻塞。调用方据此告知用户结果不完整。
    pub(crate) fn find_at(
        &self,
        text: &str,
        from: usize,
    ) -> Result<Option<(usize, usize)>, GaveUp> {
        match self {
            Pattern::Plain(regex) => Ok(regex.find_at(text, from).map(|m| (m.start(), m.end()))),
            Pattern::Fancy(regex) => match regex.find_from_pos(text, from) {
                Ok(found) => Ok(found.map(|m| (m.start(), m.end()))),
                Err(_) => Err(GaveUp),
            },
        }
    }

    /// 返回从字节偏移 `from` 起的第一个匹配及其各分组的字节范围；第 0 项是整个匹配，没有
    /// 参与匹配的分组为 `None`。
    ///
    /// # Errors
    ///
    /// 回溯超过上限时返回 [`GaveUp`]，同 [`Pattern::find_at`]。
    pub(crate) fn captures_at(&self, text: &str, from: usize) -> Result<Option<Groups>, GaveUp> {
        match self {
            Pattern::Plain(regex) => Ok(regex.captures_at(text, from).map(|captures| {
                (captures.iter())
                    .map(|group| group.map(|m| (m.start(), m.end())))
                    .collect()
            })),
            Pattern::Fancy(regex) => match regex.captures_from_pos(text, from) {
                Ok(found) => Ok(found.map(|captures| {
                    (captures.iter())
                        .map(|group| group.map(|m| (m.start(), m.end())))
                        .collect()
                })),
                Err(_) => Err(GaveUp),
            },
        }
    }

    /// 返回恰好位于 `range`（字节范围）的匹配按 `template` 展开分组引用后的文字。
    ///
    /// 该位置已不是匹配时返回 `None`。
    pub(crate) fn expand_at(
        &self,
        text: &str,
        range: (usize, usize),
        template: &str,
    ) -> Option<String> {
        let mut out = String::new();
        match self {
            Pattern::Plain(regex) => {
                let captures = regex.captures_at(text, range.0)?;
                let whole = captures.get(0)?;
                ((whole.start(), whole.end()) == range)
                    .then(|| captures.expand(template, &mut out))?;
            }
            Pattern::Fancy(regex) => {
                let captures = regex.captures_from_pos(text, range.0).ok()??;
                let whole = captures.get(0)?;
                ((whole.start(), whole.end()) == range)
                    .then(|| captures.expand(template, &mut out))?;
            }
        }
        Some(out)
    }

    /// 返回 `text` 中第一处匹配按 `template` 展开后的文字，用于结果列表显示「替换后」；
    /// 没有匹配时返回 `None`。
    pub(crate) fn expand_first(&self, text: &str, template: &str) -> Option<String> {
        let range = self.find_at(text, 0).ok()??;
        self.expand_at(text, range, template)
    }
}

/// 跨行查找中的一处匹配。
#[derive(Debug, Clone, Copy)]
pub(crate) struct JoinedMatch {
    /// 匹配起点所在的行号（从 0 起）。
    pub line: usize,
    /// 匹配终点所在的行号（从 0 起，含）；终点恰好在换行之后时不计入下一行。
    pub last: usize,
    /// 第 `line` 行在连接文本中的起始字节偏移。
    pub line_byte: usize,
    /// 匹配在连接文本中的字节范围。
    pub bytes: (usize, usize),
    /// 匹配在文档中的位置范围（字符偏移；换行按文档中的实际写法计算）。
    pub range: (usize, usize),
}

impl Matcher {
    /// 编译查询。
    ///
    /// # Errors
    ///
    /// 正则语法错误或写法不被支持时返回错误说明。
    pub(crate) fn new(query: &str, options: SearchOptions) -> Result<Self, String> {
        Self::requiring(query, options, "")
    }

    /// 编译查询，`required` 是每处匹配都含有的一段文字，大小写规则与查询相同；为空时没有这项信息。
    ///
    /// 由多种写法合并成的长正则没有可供预筛选的公共前缀，在整块文本上只能逐字节运行状态机。有
    /// 必含的文字时，只有含这段文字的行才逐行匹配，其余内容按纯文字跳过。
    ///
    /// # Errors
    ///
    /// 正则语法错误或写法不被支持时返回错误说明。
    pub(crate) fn requiring(
        query: &str,
        options: SearchOptions,
        required: &str,
    ) -> Result<Self, String> {
        let multiline = is_multiline(query, options.regex);
        let mut block = None;
        let kind = if options.case_sensitive && !options.regex && !multiline {
            Kind::Literal(Box::new(Finder::new(query.as_bytes()).into_owned()))
        } else {
            let escaped;
            let pattern = if options.regex {
                query
            } else {
                escaped = regex::escape(query);
                &escaped
            };
            // 跨行查找时 `^`、`$` 仍按行首、行尾匹配（`multi_line` 标志）。
            let regex = Pattern::new(pattern, !options.case_sensitive, multiline)?;
            let plain = match regex {
                _ if !required.is_empty() => Some(Cow::Owned(regex::escape(required))),
                Pattern::Plain(_) if !pattern.contains("\\A") && !pattern.contains("\\z") => {
                    Some(Cow::Borrowed(pattern))
                }
                Pattern::Plain(_) => None,
                Pattern::Fancy(_) => relaxed(pattern).map(Cow::Owned),
            };
            if !multiline && let Some(plain) = plain {
                block = RegexBuilder::new(&plain)
                    .case_insensitive(!options.case_sensitive)
                    .multi_line(true)
                    .crlf(true)
                    .build()
                    .ok();
            }
            Kind::Regex(regex)
        };

        Ok(Self {
            kind,
            whole_word: options.whole_word,
            regex: options.regex,
            multiline,
            block,
            gave_up: Arc::default(),
        })
    }

    /// 是否可以用 [`Self::block_find`] 在整块文本上预先查找候选位置。
    pub(crate) fn has_block(&self) -> bool {
        matches!(self.kind, Kind::Literal(_)) || self.block.is_some()
    }

    /// 在由多行连接成的 `text` 中，返回从字节偏移 `from` 起第一个候选匹配的字节偏移。
    ///
    /// 单行内的匹配在整块文本的同一行上也有候选（`^`、`$` 按行匹配，`.` 不跨换行；用到环视、
    /// 反向引用时使用放宽后的正则，有必含的文字时找这段文字），因此第一个候选之前的行一定没有
    /// 匹配；候选所在的行则不一定有匹配。没有候选，或 [`Self::has_block`] 为 `false` 时返回
    /// `None`。
    pub(crate) fn block_find(&self, text: &str, from: usize) -> Option<usize> {
        match (&self.kind, &self.block) {
            (Kind::Literal(finder), _) => finder.find(&text.as_bytes()[from..]).map(|at| from + at),
            (Kind::Regex(_), Some(block)) => block.find_at(text, from).map(|m| m.start()),
            (Kind::Regex(_), None) => None,
        }
    }

    /// 是否有匹配因回溯过多而放弃（自上次 [`Self::take_gave_up`] 以来），不清除标记。
    pub(crate) fn gave_up(&self) -> bool {
        self.gave_up.load(Ordering::Relaxed)
    }

    /// 返回上次调用以来是否有匹配因回溯过多而放弃，并清除该标记。
    fn take_gave_up(&self) -> bool {
        self.gave_up.swap(false, Ordering::Relaxed)
    }

    /// 是否跨行查找。
    pub(crate) fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// 返回正则模式下的 [`Pattern`]，替换时用该正则展开分组引用；非正则模式返回 `None`。
    fn template_regex(&self) -> Option<&Pattern> {
        match &self.kind {
            Kind::Regex(regex) if self.regex => Some(regex),
            _ => None,
        }
    }

    /// 在一行里依次查找所有不重叠的匹配，把每处匹配在 `line` 中的字节范围交给 `f`；`f` 返回
    /// `Break` 时停止并返回 `Break`。
    ///
    /// 空匹配和不满足全字条件的匹配不算。
    pub(crate) fn find_all(
        &self,
        line: &str,
        mut f: impl FnMut(usize, usize) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        // 用到环视、反向引用时先用放宽后的正则判断：该正则不匹配时该行一定没有匹配，可以跳过回溯。
        if let (Kind::Regex(Pattern::Fancy(_)), Some(block)) = (&self.kind, &self.block)
            && !block.is_match(line)
        {
            return ControlFlow::Continue(());
        }

        let bytes = line.as_bytes();
        let mut from = 0;

        while from <= bytes.len() {
            let Some((s, e)) = self.find(line, from) else {
                break;
            };

            // 跳过空匹配和不满足全字条件的匹配，从下一个字符继续查找；例如在「foofoo foo」中
            // 仍能找到最后的 `foo`。
            if s == e || (self.whole_word && !is_whole_word(bytes, s, e)) {
                from = s + char_width(bytes, s);
                continue;
            }

            f(s, e)?;
            from = e;
        }

        ControlFlow::Continue(())
    }

    /// 在一行里查找所有不重叠的匹配，位置记入 `matches`，行号记入 `lines`。
    ///
    /// `index` 是行号（从 0 起），`start` 是该行行首的位置（字符偏移）。`matches` 的长度达到
    /// `limit` 时返回 `Break`。
    fn scan(
        &self,
        index: usize,
        start: usize,
        line: &str,
        matches: &mut Vec<(usize, usize)>,
        lines: &mut Vec<usize>,
        limit: usize,
    ) -> ControlFlow<()> {
        let bytes = line.as_bytes();
        // 查找得到的是字节偏移，增量换算为字符偏移：`byte` 处的位置是 `pos`。
        let (mut byte, mut pos) = (0, start);

        self.find_all(line, |s, e| {
            let from = pos + count_chars(&bytes[byte..s]);
            (byte, pos) = (e, from + count_chars(&bytes[s..e]));
            matches.push((from, pos));

            if lines.last() != Some(&index) {
                lines.push(index);
            }

            if matches.len() >= limit {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
    }

    /// 跨行查找：在 `joined`（见 [`joined`]）中依次查找，每处匹配交给 `f`，`f` 返回 `Break`
    /// 时停止。
    ///
    /// `starts[i]` 是第 `i` 行行首在文档中的位置（字符偏移）。
    pub(crate) fn scan_joined(
        &self,
        joined: &str,
        starts: &[usize],
        mut f: impl FnMut(JoinedMatch) -> ControlFlow<()>,
    ) {
        let bytes = joined.as_bytes();
        let mut cursor = Cursor::default();
        let mut from = 0;

        while from <= bytes.len() {
            let Some((s, e)) = self.find(joined, from) else {
                break;
            };
            if s == e || (self.whole_word && !is_whole_word(bytes, s, e)) {
                from = s + char_width(bytes, s);
                continue;
            }

            let (line, line_byte, col) = cursor.seek(bytes, s);
            let (end_line, _, end_col) = cursor.seek(bytes, e);
            // 匹配终点恰好在换行之后（列 0）时，不计入终点所在的下一行。
            let last = if end_col == 0 && end_line > line {
                end_line - 1
            } else {
                end_line
            };
            let found = JoinedMatch {
                line,
                last,
                line_byte,
                bytes: (s, e),
                range: (starts[line] + col, starts[end_line] + end_col),
            };
            if f(found).is_break() {
                return;
            }
            from = e;
        }
    }

    /// 返回从字节偏移 `from` 起的第一个匹配的字节范围。
    fn find(&self, text: &str, from: usize) -> Option<(usize, usize)> {
        match &self.kind {
            Kind::Literal(finder) => finder
                .find(&text.as_bytes()[from..])
                .map(|at| (from + at, from + at + finder.needle().len())),
            Kind::Regex(regex) => regex.find_at(text, from).unwrap_or_else(|GaveUp| {
                self.gave_up.store(true, Ordering::Relaxed);
                None
            }),
        }
    }
}

/// 把用到环视、反向引用的正则放宽为 `regex` 能编译的版本，供整块文本预先查找候选使用
/// （见 [`Matcher::block_find`]）。
///
/// 放宽后的正则只会多匹配、不会少匹配：环视和 `\b` 等断言被移除（少一个条件只会多匹配），
/// 反向引用换成当前行剩余的任意字符，原子组按普通组处理。条件分支、子程序调用无法放宽，
/// 此时返回 `None`，只能逐行查找。
fn relaxed(pattern: &str) -> Option<String> {
    use fancy_regex::{Assertion, AstNode, Expr};

    fn write(expr: &Expr, out: &mut String) -> Option<()> {
        match expr {
            Expr::Empty
            | Expr::LookAround(..)
            | Expr::KeepOut
            | Expr::ContinueFromPreviousMatchEnd => out.push_str("(?:)"),
            // 保留行首、行尾断言（`\A`、`^` 都写成 `^`；整块文本按行匹配，语义恰好对应），
            // 其余断言移除。
            Expr::Assertion(
                Assertion::StartText
                | Assertion::EndText
                | Assertion::StartLine { .. }
                | Assertion::EndLine { .. },
            )
            | Expr::Literal { .. }
            | Expr::Any { .. }
            | Expr::Delegate { .. } => {
                out.push_str("(?:");
                expr.to_str(out, 0);
                out.push(')');
            }
            Expr::Assertion(_) => out.push_str("(?:)"),
            Expr::Backref { .. }
            | Expr::BackrefWithRelativeRecursionLevel { .. }
            | Expr::AstNode(AstNode::Backref { .. }, _) => out.push_str("[^\n]*"),
            Expr::Concat(children) => {
                out.push_str("(?:");
                for child in children {
                    write(child, out)?;
                }
                out.push(')');
            }
            Expr::Alt(children) => {
                out.push_str("(?:");
                for (i, child) in children.iter().enumerate() {
                    if i > 0 {
                        out.push('|');
                    }
                    write(child, out)?;
                }
                out.push(')');
            }
            Expr::Group(child) => write(child, out)?,
            Expr::AtomicGroup(child) | Expr::AstNode(AstNode::AstGroup { inner: child, .. }, _) => {
                write(child, out)?
            }
            Expr::Repeat { child, lo, hi, .. } => {
                out.push_str("(?:");
                write(child, out)?;
                out.push(')');
                match *hi {
                    usize::MAX => out.push_str(&format!("{{{lo},}}")),
                    hi => out.push_str(&format!("{{{lo},{hi}}}")),
                }
            }
            _ => return None,
        }
        Some(())
    }

    let tree = Expr::parse_tree(pattern).ok()?;
    let mut out = String::new();
    write(&tree.expr, &mut out)?;
    Some(out)
}

/// 是否需要跨行查找：查询含换行符，或正则模式下含 `\n`、`\r`、`\W`（`\W` 也能匹配换行符）。
fn is_multiline(query: &str, regex: bool) -> bool {
    if query.contains('\n') {
        return true;
    }
    let mut chars = query.chars();
    while let Some(c) = chars.next() {
        if regex && c == '\\' && matches!(chars.next(), Some('n' | 'r' | 'W')) {
            return true;
        }
    }
    false
}

/// 把整篇文本的各行用 `\n` 连接，返回连接结果和每行行首的位置（字符偏移）。
///
/// 文档中的 `\r\n`、`\r` 在连接结果里都是 `\n`，使查询中的 `\n` 能匹配任意一种换行；
/// 匹配结果再按行首位置换算回文档位置。供跨行查找使用。
pub(crate) fn joined(text: &impl Lines) -> (String, Vec<usize>) {
    let (mut joined, mut starts) = (String::new(), Vec::new());
    text.visit_lines(0, usize::MAX, |_, start, line| {
        if !starts.is_empty() {
            joined.push('\n');
        }
        joined.push_str(line);
        starts.push(start);
        ControlFlow::Continue(())
    });
    (joined, starts)
}

/// 在连接文本中自前向后移动，同时统计当前行号和行内的字符数（列号）。
#[derive(Default)]
struct Cursor {
    byte: usize,
    line: usize,
    line_byte: usize,
    col: usize,
}

impl Cursor {
    /// 向后移动到字节偏移 `to`，返回所在行的行号、该行起始字节偏移和列号。
    ///
    /// 行号与列号从 0 起，列号按字符计。
    fn seek(&mut self, bytes: &[u8], to: usize) -> (usize, usize, usize) {
        while let Some(at) = memchr::memchr(b'\n', &bytes[self.byte..to]) {
            self.byte += at + 1;
            self.line += 1;
            self.line_byte = self.byte;
            self.col = 0;
        }
        self.col += count_chars(&bytes[self.byte..to]);
        self.byte = to;
        (self.line, self.line_byte, self.col)
    }
}

/// 返回 UTF-8 字节串中的字符数（个）。
pub(crate) fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| (b as i8) >= -0x40).count()
}

/// 按首字节判断位置 `i` 处 UTF-8 字符的字节数（字节）。
///
/// `i` 位于末尾或之后时返回 1，使调用方的循环能越过末尾而终止。
fn char_width(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(0xF0..) => 4,
        Some(0xE0..) => 3,
        Some(0xC0..) => 2,
        _ => 1,
    }
}

fn char_after(bytes: &[u8], i: usize) -> Option<char> {
    let end = (i + char_width(bytes, i)).min(bytes.len());
    std::str::from_utf8(bytes.get(i..end)?).ok()?.chars().next()
}

fn char_before(bytes: &[u8], i: usize) -> Option<char> {
    let start = bytes[..i].iter().rposition(|&b| (b as i8) >= -0x40)?;
    std::str::from_utf8(&bytes[start..i]).ok()?.chars().next()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 是否位置范围 `s` 到 `e`（字节偏移，左闭右开）构成一个整词。
///
/// 匹配的某一端本身不是单词字符时（例如查询「->」），该端不检查。
fn is_whole_word(bytes: &[u8], s: usize, e: usize) -> bool {
    let separated = |outside: Option<char>, inside: Option<char>| {
        !outside.is_some_and(is_word_char) || !inside.is_some_and(is_word_char)
    };

    separated(char_before(bytes, s), char_after(bytes, s))
        && separated(char_after(bytes, e), char_before(bytes, e))
}

#[cfg(test)]
mod tests;
