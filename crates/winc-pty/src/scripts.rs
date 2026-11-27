//! shell 集成脚本的内容。
//!
//! 本机的 shell 由辅助进程在启动前写好脚本并注入。

use std::borrow::Cow;

/// zsh 的启动脚本：`.zshenv` 把 `ZDOTDIR` 恢复成用户原来的值，`winc.zsh` 安装钩子。
pub const ZSH: &[(&str, &str)] = &[
    ("zsh/.zshenv", include_str!("../shell/zshenv.zsh")),
    ("zsh/winc.zsh", include_str!("../shell/winc.zsh")),
];
/// bash 的启动脚本，由 `--rcfile` 读入。
pub const BASH: &[(&str, &str)] = &[("bash/winc.bash", include_str!("../shell/winc.bash"))];
/// fish 的启动脚本，放在 `vendor_conf.d` 里由 fish 自动读入。
pub const FISH: &[(&str, &str)] = &[(
    "fish/fish/vendor_conf.d/winc.fish",
    include_str!("../shell/winc.fish"),
)];
/// Python 环境的加载脚本：在用户配置读完后调整环境，并提供 `deactivate`。
pub const PYTHON: &[(&str, &str)] = &[
    ("python.sh", include_str!("../shell/python.sh")),
    ("python.fish", include_str!("../shell/python.fish")),
];
/// 按用户的登录 shell 设置环境变量和参数、启动它并注入上面各脚本的脚本；第一个参数是脚本所在
/// 的目录。用于 WSL 发行版。
pub const LAUNCH: &[(&str, &str)] = &[("wsl.sh", include_str!("../shell/wsl.sh"))];

/// 返回脚本 `content` 写出去的内容：换行符统一为 LF，本来就是 LF 时不复制。
///
/// `include_str!` 内嵌的是检出时的字节，Windows 上 `core.autocrlf=true` 时检出为 CRLF，而读
/// 这些脚本的 shell（WSL、dash）把回车当作词的一部分，`case … in` 会解析失败。仓库的
/// `.gitattributes` 已让这些脚本总以 LF 检出，这里兜住规则之前的检出。
pub fn unix_lines(content: &str) -> Cow<'_, str> {
    match content.contains('\r') {
        true => Cow::Owned(content.replace("\r\n", "\n")),
        false => Cow::Borrowed(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_written_with_unix_endings() {
        // 以 CRLF 检出时内嵌的脚本：写出去的是 LF；本来就是 LF 的不复制。
        let crlf = "case ${shell##*/} in\r\nzsh)\r\n";
        assert_eq!(unix_lines(crlf), "case ${shell##*/} in\nzsh)\n");
        assert!(matches!(unix_lines("a\nb\n"), Cow::Borrowed(_)));
        for (_, content) in [ZSH, BASH, FISH, PYTHON, LAUNCH].into_iter().flatten() {
            assert!(!unix_lines(content).contains('\r'));
        }
    }
}
