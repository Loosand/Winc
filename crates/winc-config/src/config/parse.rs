//! 配置文件中各类单值与集合的 JSON 读入校验与转换。

use std::collections::BTreeMap;

use winc_i18n::t;

use crate::json::Json;

/// 把「远端目录 → 本机目录」的对象（`remote.localFolders`）读入 `out`，无法理解的地方记入
/// `problems`，文案中用 `name` 指代这一项。
///
/// 键要写成 `目标:/路径`（远端目录的显示写法），按 [`remote_folder_key`] 规范；值去掉首尾
/// 空白，空字符串视为没写。规范后相同的两个键只取第一个，其余记为问题。
pub(super) fn read_local_folders(
    value: &Json,
    name: &str,
    out: &mut BTreeMap<String, String>,
    problems: &mut Vec<String>,
) {
    let Some(folders) = value.as_object() else {
        problems.push(t!("config-not-object", name = name));
        return;
    };
    for (remote, local) in folders {
        let entry = format!("{name}.{remote}");
        let local = match local.as_str().map(str::trim) {
            Some("") => continue,
            Some(local) => local.to_owned(),
            None => {
                problems.push(t!("config-not-string", name = entry));
                continue;
            }
        };
        let Some(remote) = remote_folder_key(remote) else {
            problems.push(t!("config-remote-folder-invalid", name = entry));
            continue;
        };
        if out.contains_key(&remote) {
            problems.push(t!("config-remote-folder-duplicate", name = entry));
            continue;
        }
        out.insert(remote, local);
    }
}

/// 把 `remote.localFolders` 的键规范为 `目标:/路径`：路径中的空段和 `.` 去掉、`..` 上移一级，
/// 与远端位置的比较方式相同（根目录是 `/`）。不是这种写法、目标不是有效的连接目标时返回
/// `None`。目标不含 `/`，第一个 `:/` 就是目标与路径的分界（目标可以带 `:端口`）。
fn remote_folder_key(key: &str) -> Option<String> {
    let key = key.trim();
    let at = key.find(":/")?;
    let target = winc_backend::Target::new(&key[..at])?;
    let mut segments: Vec<&str> = Vec::new();
    for segment in key[at + 1..].split('/') {
        match segment {
            "" | "." => {}
            ".." => _ = segments.pop(),
            name => segments.push(name),
        }
    }
    Some(format!("{}:/{}", target.as_str(), segments.join("/")))
}

/// 把「语言 → 命令」的对象（`format.commands`、`format.fixCommands`）读入 `out`，无法理解的地方
/// 记入 `problems`，文案中用 `name` 指代这一项。
///
/// 命令去掉首尾空白，空字符串视为没写，自动选择工具。两个键指向同一种语言时（如 `JSON` 和
/// `json`），按键名排序后靠后的一条生效。
pub(super) fn read_commands(
    value: &Json,
    name: &str,
    out: &mut BTreeMap<String, String>,
    problems: &mut Vec<String>,
) {
    let Some(commands) = value.as_object() else {
        problems.push(t!("config-not-object", name = name));
        return;
    };
    for (language, command) in commands {
        let Some(id) = language_id(language, name, problems) else {
            continue;
        };
        let command = match command.as_str().map(str::trim) {
            Some("") => continue,
            Some(command) => command.to_owned(),
            None => {
                problems.push(t!("config-not-string", name = format!("{name}.{language}")));
                continue;
            }
        };
        if out.insert(id, command).is_some() {
            let language = language.as_str();
            problems.push(t!(
                "config-duplicate-language",
                name = name,
                language = language
            ));
        }
    }
}

/// 把「语言 → 命令」写成 JSON 对象，没有命令时返回 `None`，不写出。
pub(super) fn commands_json(commands: &BTreeMap<String, String>) -> Option<Json> {
    let pairs = commands
        .iter()
        .map(|(id, command)| (id.as_str(), Json::String(command.clone())));
    (!commands.is_empty()).then(|| Json::object(pairs))
}

/// 把用户写的语言（id 或名字）转成语言的 id，无法识别时记入 `problems` 并返回 `None`。
///
/// 文案中用 `name` 指代所在的设置。
pub(super) fn language_id(
    language: &str,
    name: &str,
    problems: &mut Vec<String>,
) -> Option<String> {
    let id = winc_syntax::find(language).map(|l| l.id.to_owned());
    if id.is_none() {
        problems.push(t!(
            "config-unknown-language",
            name = name,
            language = language
        ));
    }
    id
}

/// 读取字符串设置，类型不符或只有空白时向 `problems` 记录问题并返回 `None`。
pub(super) fn read_string(value: &Json, key: &str, problems: &mut Vec<String>) -> Option<String> {
    match value.as_str() {
        Some(text) if !text.trim().is_empty() => Some(text.to_string()),
        Some(_) => {
            problems.push(t!("config-empty", name = key));
            None
        }
        None => {
            problems.push(t!("config-not-string", name = key));
            None
        }
    }
}

/// 读取布尔设置，类型不符时向 `problems` 记录问题并返回 `None`。
pub(super) fn read_bool(value: &Json, key: &str, problems: &mut Vec<String>) -> Option<bool> {
    let value = value.as_bool();
    if value.is_none() {
        problems.push(t!("config-not-bool", name = key));
    }
    value
}

/// 读取数字设置并检查范围，类型不符或超出 `range` 时向 `problems` 记录问题并返回 `None`。
///
/// 整数写法同样接受（如 `"size": 14`）；`label` 用于问题文案中的名称。
pub(super) fn read_number(
    value: &Json,
    key: &str,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    problems: &mut Vec<String>,
) -> Option<f32> {
    let Some(number) = value.as_number() else {
        problems.push(t!("config-not-number", name = key));
        return None;
    };

    if !number.is_finite() || !range.contains(&(number as f32)) {
        let setting = format!("{key} {number}");
        let range = format!("{}–{}", range.start(), range.end());
        problems.push(t!(
            "config-out-of-range",
            setting = setting,
            label = label,
            range = range
        ));
        return None;
    }

    Some(number as f32)
}
