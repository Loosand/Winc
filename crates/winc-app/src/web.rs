//! 取回网络上的文件：运行系统的 `curl`，未安装时运行 `wget`，本程序不链接网络库。
//!
//! 应用内更新、更新日志和 Markdown 网络图片都经过这里，跟随系统的代理设置。

use std::io;
use std::process::{Command, Output, Stdio};

use winc_i18n::t;

/// 构造运行 `program` 的命令。
#[cfg(unix)]
pub(crate) fn command(program: &str) -> Command {
    Command::new(program)
}

/// 构造运行 `program` 的命令，不为它创建控制台窗口。
#[cfg(windows)]
pub(crate) fn command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`：不为控制台程序创建窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// 运行 `curl` 的命令：跟随系统的代理，按代理的要求选认证方式；Windows 上查不到证书的吊销状态
/// 时不算失败，证书链、主机名和有效期照常验证。
pub(crate) fn curl() -> Command {
    let mut curl = command("curl");
    curl.args(["-q", "--proxy-anyauth"]);
    #[cfg(windows)]
    if revoke_best_effort() {
        curl.arg("--ssl-revoke-best-effort");
    }
    winc_editor::proxy::apply(&mut curl);
    curl
}

/// 运行 `wget` 的命令：跟随系统的代理。
pub(crate) fn wget() -> Command {
    let mut wget = command("wget");
    winc_editor::proxy::apply(&mut wget);
    wget
}

/// 本机的 `curl` 是否认得 `--ssl-revoke-best-effort`（7.70 起才有），只试一次。
#[cfg(windows)]
fn revoke_best_effort() -> bool {
    static KNOWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KNOWN.get_or_init(|| {
        command("curl")
            .args(["--ssl-revoke-best-effort", "--version"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// 联网请求失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 连不上代理服务器。
    Proxy,
    /// 代理服务器要求认证，或认证没有通过。
    ProxyAuth,
    /// 服务器的证书无法验证。
    Certificate,
    /// 超过了等待时间。
    Timeout,
    /// 连不上服务器。
    Network,
    /// 服务器返回了错误。
    Server,
    /// 取回的内容不对。
    Invalid,
    /// 其他原因：命令的错误输出，去掉了其中的用户名和密码。
    Other(String),
}

impl Failure {
    /// 按 `curl` 的退出码 `code` 和错误输出 `stderr` 归类；`proxied` 是请求是否经过代理。
    pub(crate) fn from_curl(code: Option<i32>, stderr: &str, proxied: bool) -> Failure {
        let proxy = stderr.to_ascii_lowercase().contains("proxy");
        match code {
            _ if stderr.contains("407") => Failure::ProxyAuth,
            Some(5 | 97) => Failure::Proxy,
            Some(7) if proxied => Failure::Proxy,
            Some(56) if proxied && proxy => Failure::Proxy,
            Some(6 | 7 | 52 | 55 | 56) => Failure::Network,
            Some(22) => Failure::Server,
            Some(28) => Failure::Timeout,
            Some(35 | 51 | 58 | 60 | 77 | 83 | 90 | 91) => Failure::Certificate,
            _ => Failure::other(stderr, code),
        }
    }

    /// 按 `wget` 的退出码 `code` 归类。
    fn from_wget(code: Option<i32>, stderr: &str) -> Failure {
        match code {
            _ if stderr.contains("407") => Failure::ProxyAuth,
            Some(4) => Failure::Network,
            Some(5) => Failure::Certificate,
            Some(8) => Failure::Server,
            _ => Failure::other(stderr, code),
        }
    }

    /// 没有归类的失败：错误输出的第一行，没有输出时为退出码。
    fn other(stderr: &str, code: Option<i32>) -> Failure {
        let line = stderr.lines().map(str::trim).find(|line| !line.is_empty());
        let code = code.map_or_else(|| "?".to_owned(), |code| code.to_string());
        Failure::Other(line.map_or_else(|| format!("exit {code}"), redact))
    }

    /// 给用户看的原因。
    pub fn message(&self) -> String {
        match self {
            Failure::Proxy => t!("net-proxy").to_owned(),
            Failure::ProxyAuth => t!("net-proxy-auth").to_owned(),
            Failure::Certificate => t!("net-certificate").to_owned(),
            Failure::Timeout => t!("net-timeout").to_owned(),
            Failure::Network => t!("net-network").to_owned(),
            Failure::Server => t!("net-server").to_owned(),
            Failure::Invalid => t!("net-invalid").to_owned(),
            Failure::Other(message) => message.clone(),
        }
    }
}

/// 去掉 `text` 中网址里的用户名和密码（`scheme://用户:密码@主机` 中的 `用户:密码@`）。
pub(crate) fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(tail.len());
        rest = match tail[..end].rfind('@') {
            Some(user) => &tail[user + 1..],
            None => tail,
        };
    }
    out.push_str(rest);
    out
}

/// 取回 `url` 的内容，最长等待 `timeout` 秒：优先使用 `curl`，未安装时使用 `wget`。
///
/// `limit` 是内容长度的上限（字节）：服务器声明的长度超过它时 `curl` 不下载，取回的内容超过它
/// 时丢弃。
///
/// # Errors
///
/// 请求失败或超过上限时返回失败的原因。
pub(crate) fn fetch(url: &str, timeout: u64, limit: Option<u64>) -> Result<Vec<u8>, Failure> {
    let timeout = timeout.to_string();
    let within = limit.map(|limit| limit.to_string());
    let bound = within
        .iter()
        .flat_map(|limit| ["--max-filesize", limit.as_str()]);
    let mut curl = curl();
    let proxied = winc_editor::proxy::configured(&curl);
    let output = curl
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--max-time", &timeout])
        .args(bound)
        .arg(url)
        .stdin(Stdio::null())
        .output();
    let (output, wget) = match output {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let output = wget()
                .args(["--quiet", "--timeout", &timeout, "--output-document", "-"])
                .arg(url)
                .stdin(Stdio::null())
                .output();
            (output, true)
        }
        output => (output, false),
    };
    let output = output.map_err(|e| Failure::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(failure(&output, wget, proxied));
    }
    let fits = limit.is_none_or(|limit| output.stdout.len() as u64 <= limit);
    fits.then_some(output.stdout).ok_or(Failure::Invalid)
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum HttpFailure {
    Status(u16),
    Curl(i32, bool),
    MissingCurl,
    ProxyAuth,
    Invalid,
    NoArticle,
}

impl HttpFailure {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Status(code @ (401 | 403 | 429)) => {
                t!("reader-site-blocked", code = code)
            }
            Self::Status(code) => t!("reader-http-status", code = code),
            Self::Curl(code, proxied) => Failure::from_curl(Some(code), "", proxied).message(),
            Self::MissingCurl => t!("reader-curl-unavailable").to_owned(),
            Self::ProxyAuth => Failure::ProxyAuth.message(),
            Self::Invalid => t!("net-invalid").to_owned(),
            Self::NoArticle => t!("reader-no-article").to_owned(),
        }
    }
}

/// Bounded HTTP(S) download for user-supplied reading URLs. Resolve relative article
/// links against the final URL rather than the address before redirects.
pub(crate) fn fetch_http(
    url: &url::Url,
    seconds: u64,
    limit: u64,
) -> Result<(url::Url, Vec<u8>), HttpFailure> {
    fetch_http_with_referer(url, seconds, limit, None)
}

/// Like `fetch_http`, but image hosts may need to know the origin of the article.
pub(crate) fn fetch_http_with_referer(
    url: &url::Url,
    seconds: u64,
    limit: u64,
    referer: Option<&str>,
) -> Result<(url::Url, Vec<u8>), HttpFailure> {
    use std::io::Read;

    let mut curl = curl();
    #[cfg(test)]
    if url.host_str() == Some("127.0.0.1") {
        curl.args(["--noproxy", "127.0.0.1"]);
    }
    if let Some(referer) = referer {
        curl.args(["--referer", referer]);
    }
    let proxied = winc_editor::proxy::configured(&curl);
    let mut child = curl
        .args(["--fail", "--silent", "--location", "--globoff"])
        .args([
            "--proto",
            "=http,https",
            "--proto-redir",
            image_redirect_protocols(referer),
        ])
        .args(["--max-redirs", "5", "--max-time", &seconds.to_string()])
        .args([
            "--max-filesize",
            &limit.to_string(),
            "--write-out",
            "\n%{http_code} %{http_connect} %{url_effective}",
        ])
        .args(["--", url.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                HttpFailure::MissingCurl
            } else {
                HttpFailure::Invalid
            }
        })?;
    let mut output = Vec::new();
    let cap = limit + 8192;
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(cap)
        .read_to_end(&mut output);
    if read.is_err() || output.len() as u64 >= cap {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HttpFailure::Invalid);
    }
    let status = child.wait().map_err(|_| HttpFailure::Invalid)?;
    let trailer = output.iter().rposition(|&b| b == b'\n');
    let mut meta = trailer
        .and_then(|i| std::str::from_utf8(&output[i + 1..]).ok())
        .unwrap_or_default()
        .splitn(3, ' ');
    let http = meta.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let connect = meta.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    if !status.success() {
        return Err(if connect == 407 || http == 407 {
            HttpFailure::ProxyAuth
        } else if connect >= 400 {
            HttpFailure::Curl(97, true)
        } else if http >= 400 {
            HttpFailure::Status(http)
        } else if status.code() == Some(63) {
            HttpFailure::Invalid
        } else {
            HttpFailure::Curl(status.code().unwrap_or(-1), proxied)
        });
    }
    let split = trailer
        .filter(|&i| i as u64 <= limit)
        .ok_or(HttpFailure::Invalid)?;
    let effective = meta
        .next()
        .and_then(|url| url::Url::parse(url).ok())
        .ok_or(HttpFailure::Invalid)?;
    output.truncate(split);
    Ok((effective, output))
}

/// Use only the article's origin, never its path or query. Do not send it to an
/// unencrypted subresource when it came from an HTTPS page.
pub(crate) fn image_referer(page: &url::Url, image: &url::Url) -> Option<String> {
    if !matches!(image.scheme(), "http" | "https")
        || !matches!(page.scheme(), "http" | "https")
        || (page.scheme() == "https" && image.scheme() != "https")
    {
        return None;
    }
    Some(format!("{}/", page.origin().ascii_serialization()))
}

/// curl forwards an explicit referer across redirects; keep HTTPS image
/// requests with that header from ever downgrading to HTTP.
pub(crate) fn image_redirect_protocols(referer: Option<&str>) -> &'static str {
    if referer.is_some_and(|r| r.starts_with("https://")) {
        "=https"
    } else {
        "=http,https"
    }
}

/// 下载命令（`wget` 为真时是 `wget`，否则是 `curl`）失败的原因；`proxied` 见
/// [`Failure::from_curl`]。
pub(crate) fn failure(output: &Output, wget: bool, proxied: bool) -> Failure {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code();
    if wget {
        Failure::from_wget(code, &stderr)
    } else {
        Failure::from_curl(code, &stderr, proxied)
    }
}

#[cfg(test)]
mod tests;
