//! Settings shown for parity review. A Beta row is descriptive only: it has no
//! persisted value or control until its editing behavior is wired up.

use winc_i18n::t;

macro_rules! beta_settings {
    ($($page:ident { $($setting:ident => $label:literal $(, $desc:literal)? ;)* })*) => {
        /// A not-yet-configurable writing or Markdown option.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BetaSetting { $($($setting,)*)* }

        impl BetaSetting {
            pub(super) fn is_markdown(self) -> bool {
                match self { $($(Self::$setting => beta_settings!(@markdown $page),)*)* }
            }

            pub(super) fn label(self) -> &'static str {
                match self { $($(Self::$setting => t!($label),)*)* }
            }

            pub(super) fn description(self) -> Option<&'static str> {
                match self { $($(Self::$setting => beta_settings!(@desc $($desc)?),)*)* }
            }
        }
    };
    (@markdown Writing) => { false };
    (@markdown Markdown) => { true };
    (@desc $desc:literal) => { Some(t!($desc)) };
    (@desc) => { None };
}

beta_settings! {
    Writing {
        Spellcheck => "settings-writing-spellcheck", "settings-writing-spellcheck-desc";
        Autocorrect => "settings-writing-autocorrect";
        SpellcheckLanguage => "settings-writing-spellcheck-language";
        PairBrackets => "settings-writing-pair-brackets", "settings-writing-pair-brackets-desc";
        PairMarkdown => "settings-writing-pair-markdown";
        Direction => "settings-writing-direction", "settings-writing-direction-desc";
        SmartQuotes => "settings-writing-smart-quotes";
        DoubleQuotes => "settings-writing-double-quotes";
        SingleQuotes => "settings-writing-single-quotes";
        SmartDashes => "settings-writing-smart-dashes";
        PunctuationTiming => "settings-writing-punctuation-timing";
        ConvertPunctuation => "settings-writing-convert-punctuation", "settings-writing-convert-punctuation-desc";
        TextReplacement => "settings-writing-text-replacement";
        CjkPairs => "settings-writing-cjk-pairs", "settings-writing-cjk-pairs-desc";
        CornerQuotes => "settings-writing-corner-quotes";
        CjkSpacing => "settings-writing-cjk-spacing";
        CjkMarkdown => "settings-writing-cjk-markdown", "settings-writing-cjk-markdown-desc";
        ZenMode => "settings-writing-zen-mode", "settings-writing-zen-mode-desc";
        ZenHideTitlebar => "settings-writing-zen-hide-titlebar";
        ZenRevealTitlebar => "settings-writing-zen-reveal-titlebar";
        ZenHideToolbar => "settings-writing-zen-hide-toolbar";
        ZenFocus => "settings-writing-zen-focus";
        ZenTypewriter => "settings-writing-zen-typewriter";
        ZenFlow => "settings-writing-zen-flow";
        ZenFullscreen => "settings-writing-zen-fullscreen";
        TypewriterWithFocus => "settings-writing-typewriter-with-focus";
        FocusWithTypewriter => "settings-writing-focus-with-typewriter";
        FlowMode => "settings-writing-flow-mode", "settings-writing-flow-mode-desc";
        FlowDisableSpellcheck => "settings-writing-flow-disable-spellcheck";
    }
    Markdown {
        Strict => "settings-syntax-strict", "settings-syntax-strict-desc";
        Containers => "settings-syntax-containers", "settings-syntax-containers-desc";
        Subscript => "settings-syntax-subscript";
        Superscript => "settings-syntax-superscript";
        Highlight => "settings-syntax-highlight", "settings-syntax-highlight-desc";
        Wiki => "settings-syntax-wiki", "settings-syntax-wiki-desc";
        Comments => "settings-syntax-comments", "settings-syntax-comments-desc";
        CriticMarkup => "settings-syntax-criticmarkup";
        HeadingStyle => "settings-syntax-heading-style";
        BulletMarker => "settings-syntax-bullet-marker";
        Citations => "settings-syntax-citations", "settings-syntax-citations-desc";
        Bibliography => "settings-syntax-bibliography";
        CitationStyle => "settings-syntax-citation-style";
        Zotero => "settings-syntax-zotero";
        InlineMath => "settings-math-inline", "settings-math-inline-desc";
        LatexDelimiters => "settings-math-latex-delimiters", "settings-math-latex-delimiters-desc";
        FencedMath => "settings-math-fenced", "settings-math-fenced-desc";
        Physics => "settings-math-physics";
        LegacyMath => "settings-math-legacy";
        EquationNumbers => "settings-math-equation-numbers";
        MathHtml => "settings-math-html";
        TexMacros => "settings-math-macros";
        MathExtensions => "settings-math-extensions", "settings-math-extensions-desc";
        CodeLineNumbers => "settings-code-line-numbers", "settings-code-line-numbers-desc";
        CodeWrap => "settings-code-wrap", "settings-code-wrap-desc";
        CustomFormats => "settings-syntax-custom-formats", "settings-syntax-custom-formats-desc";
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
    TextWidth,
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
            [
                WritingItem::RemoteImages,
                WritingItem::TextWidth,
                WritingItem::LineNumbers,
            ],
        ),
    ]
}
