//! Settings shown for parity review. A Beta row is descriptive only: it has no
//! persisted value or control until its editing behavior is wired up.

use winc_i18n::t;

macro_rules! beta_settings {
    ($($page:ident { $($setting:ident => $label:expr $(, $desc:expr)? ;)* })*) => {
        /// A not-yet-configurable writing or Markdown option.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BetaSetting { $($($setting,)*)* }

        impl BetaSetting {
            pub(super) fn is_markdown(self) -> bool {
                match self { $($(Self::$setting => beta_settings!(@markdown $page),)*)* }
            }

            pub(super) fn label(self) -> &'static str {
                match self { $($(Self::$setting => $label,)*)* }
            }

            pub(super) fn description(self) -> Option<&'static str> {
                match self { $($(Self::$setting => beta_settings!(@desc $($desc)?),)*)* }
            }
        }
    };
    (@markdown Writing) => { false };
    (@markdown Markdown) => { true };
    (@desc $desc:expr) => { Some($desc) };
    (@desc) => { None };
}

beta_settings! {
    Writing {
        Spellcheck => t!("settings-writing-spellcheck"), t!("settings-writing-spellcheck-desc");
        Autocorrect => t!("settings-writing-autocorrect");
        SpellcheckLanguage => t!("settings-writing-spellcheck-language");
        PairBrackets => t!("settings-writing-pair-brackets"), t!("settings-writing-pair-brackets-desc");
        PairMarkdown => t!("settings-writing-pair-markdown");
        Direction => t!("settings-writing-direction"), t!("settings-writing-direction-desc");
        SmartQuotes => t!("settings-writing-smart-quotes");
        DoubleQuotes => t!("settings-writing-double-quotes");
        SingleQuotes => t!("settings-writing-single-quotes");
        SmartDashes => t!("settings-writing-smart-dashes");
        PunctuationTiming => t!("settings-writing-punctuation-timing");
        ConvertPunctuation => t!("settings-writing-convert-punctuation"), t!("settings-writing-convert-punctuation-desc");
        TextReplacement => t!("settings-writing-text-replacement");
        CjkPairs => t!("settings-writing-cjk-pairs"), t!("settings-writing-cjk-pairs-desc");
        CornerQuotes => t!("settings-writing-corner-quotes");
        CjkSpacing => t!("settings-writing-cjk-spacing");
        CjkMarkdown => t!("settings-writing-cjk-markdown"), t!("settings-writing-cjk-markdown-desc");
        ZenMode => t!("settings-writing-zen-mode"), t!("settings-writing-zen-mode-desc");
        ZenHideTitlebar => t!("settings-writing-zen-hide-titlebar");
        ZenRevealTitlebar => t!("settings-writing-zen-reveal-titlebar");
        ZenHideToolbar => t!("settings-writing-zen-hide-toolbar");
        ZenFocus => t!("settings-writing-zen-focus");
        ZenTypewriter => t!("settings-writing-zen-typewriter");
        ZenFlow => t!("settings-writing-zen-flow");
        ZenFullscreen => t!("settings-writing-zen-fullscreen");
        TypewriterWithFocus => t!("settings-writing-typewriter-with-focus");
        FocusWithTypewriter => t!("settings-writing-focus-with-typewriter");
        FlowMode => t!("settings-writing-flow-mode"), t!("settings-writing-flow-mode-desc");
        FlowDisableSpellcheck => t!("settings-writing-flow-disable-spellcheck");
    }
    Markdown {
        Strict => t!("settings-syntax-strict"), t!("settings-syntax-strict-desc");
        Containers => t!("settings-syntax-containers"), t!("settings-syntax-containers-desc");
        Subscript => t!("settings-syntax-subscript");
        Superscript => t!("settings-syntax-superscript");
        Highlight => t!("settings-syntax-highlight"), t!("settings-syntax-highlight-desc");
        Wiki => t!("settings-syntax-wiki"), t!("settings-syntax-wiki-desc");
        Comments => t!("settings-syntax-comments"), t!("settings-syntax-comments-desc");
        CriticMarkup => t!("settings-syntax-criticmarkup");
        HeadingStyle => t!("settings-syntax-heading-style");
        BulletMarker => t!("settings-syntax-bullet-marker");
        Citations => t!("settings-syntax-citations"), t!("settings-syntax-citations-desc");
        Bibliography => t!("settings-syntax-bibliography");
        CitationStyle => t!("settings-syntax-citation-style");
        Zotero => t!("settings-syntax-zotero");
        InlineMath => t!("settings-math-inline"), t!("settings-math-inline-desc");
        LatexDelimiters => t!("settings-math-latex-delimiters"), t!("settings-math-latex-delimiters-desc");
        FencedMath => t!("settings-math-fenced"), t!("settings-math-fenced-desc");
        Physics => t!("settings-math-physics");
        LegacyMath => t!("settings-math-legacy");
        EquationNumbers => t!("settings-math-equation-numbers");
        MathHtml => t!("settings-math-html");
        TexMacros => t!("settings-math-macros");
        MathExtensions => t!("settings-math-extensions"), t!("settings-math-extensions-desc");
        CodeLineNumbers => t!("settings-code-line-numbers"), t!("settings-code-line-numbers-desc");
        CodeWrap => t!("settings-code-wrap"), t!("settings-code-wrap-desc");
        CustomFormats => t!("settings-syntax-custom-formats"), t!("settings-syntax-custom-formats-desc");
    }
}

/// 写作页与 Markdown 页分组表中的单项，由父模块映射为 `Setting`。
#[derive(Debug, Clone, Copy)]
pub(super) enum WritingItem {
    Beta(BetaSetting),
    Shortcuts,
    SelectionToolbar,
    Typewriter,
    Focus,
    RemoteImages,
    LineNumbers,
}

fn group(
    title: &'static str,
    items: impl IntoIterator<Item = WritingItem>,
) -> (&'static str, Vec<WritingItem>) {
    (title, items.into_iter().collect())
}

fn beta_group(title: &'static str, items: &[BetaSetting]) -> (&'static str, Vec<WritingItem>) {
    group(title, items.iter().copied().map(WritingItem::Beta))
}

pub(super) fn writing_groups() -> Vec<(&'static str, Vec<WritingItem>)> {
    use BetaSetting::*;
    use WritingItem::Beta;
    vec![
        beta_group(
            t!("settings-section-spelling"),
            &[Spellcheck, Autocorrect, SpellcheckLanguage],
        ),
        group(
            t!("settings-section-writing-input"),
            [
                Beta(PairBrackets),
                Beta(PairMarkdown),
                WritingItem::Shortcuts,
                WritingItem::SelectionToolbar,
            ],
        ),
        beta_group(t!("settings-section-writing-direction"), &[Direction]),
        beta_group(
            t!("settings-section-smart-punctuation"),
            &[
                SmartQuotes,
                DoubleQuotes,
                SingleQuotes,
                SmartDashes,
                PunctuationTiming,
                ConvertPunctuation,
                TextReplacement,
            ],
        ),
        beta_group(
            t!("settings-section-cjk-typography"),
            &[CjkPairs, CornerQuotes, CjkSpacing, CjkMarkdown],
        ),
        beta_group(
            t!("settings-section-zen-mode"),
            &[
                ZenMode,
                ZenHideTitlebar,
                ZenRevealTitlebar,
                ZenHideToolbar,
                ZenFocus,
                ZenTypewriter,
                ZenFlow,
                ZenFullscreen,
            ],
        ),
        group(
            t!("settings-markdown-typewriter"),
            [WritingItem::Typewriter, Beta(TypewriterWithFocus)],
        ),
        group(
            t!("settings-markdown-focus"),
            [WritingItem::Focus, Beta(FocusWithTypewriter)],
        ),
        beta_group(
            t!("settings-section-flow-mode"),
            &[FlowMode, FlowDisableSpellcheck],
        ),
    ]
}

pub(super) fn markdown_groups() -> Vec<(&'static str, Vec<WritingItem>)> {
    use BetaSetting::*;
    vec![
        beta_group(
            t!("settings-section-markdown-syntax"),
            &[
                Strict,
                Containers,
                Subscript,
                Superscript,
                Highlight,
                Wiki,
                Comments,
                CriticMarkup,
                HeadingStyle,
                BulletMarker,
            ],
        ),
        beta_group(
            t!("settings-section-citations"),
            &[Citations, Bibliography, CitationStyle, Zotero],
        ),
        beta_group(
            t!("settings-section-math"),
            &[
                InlineMath,
                LatexDelimiters,
                FencedMath,
                Physics,
                LegacyMath,
                EquationNumbers,
                MathHtml,
                TexMacros,
            ],
        ),
        beta_group(t!("settings-math-extensions"), &[MathExtensions]),
        beta_group(
            t!("settings-section-code-blocks"),
            &[CodeLineNumbers, CodeWrap],
        ),
        beta_group(t!("settings-section-custom-formats"), &[CustomFormats]),
        group(
            t!("settings-section-markdown-display"),
            [WritingItem::RemoteImages, WritingItem::LineNumbers],
        ),
    ]
}
