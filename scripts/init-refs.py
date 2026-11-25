#!/usr/bin/env python3
"""Initialize local gitignored reference repositories listed in Minc blueprints."""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import datetime, timezone
import os
from pathlib import Path
import shutil
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DIR = ROOT / "references"


@dataclass(frozen=True)
class RefRepo:
    name: str
    url: str
    groups: tuple[str, ...]
    focus: str
    key_paths: tuple[str, ...] = ()


REPOS: tuple[RefRepo, ...] = (
    # --- Shared (CPUI + Rich-Text Editor) ---
    RefRepo(
        name="zed",
        url="https://github.com/zed-industries/zed.git",
        groups=("cpui", "richtext-editor"),
        focus="GPUI（声明式界面、弹性布局、自绘区域）与 Zed Editor（显示映射、虚拟区块）",
        key_paths=("crates/gpui", "crates/editor", "crates/text", "crates/multi_buffer"),
    ),
    # --- CPUI Blueprint (docs/bluemap/cpui.md) ---
    RefRepo(
        name="egui",
        url="https://github.com/emilk/egui.git",
        groups=("cpui",),
        focus="即时模式界面与动作驱动的交互",
        key_paths=("crates/egui",),
    ),
    RefRepo(
        name="iced",
        url="https://github.com/iced-rs/iced.git",
        groups=("cpui",),
        focus="状态与视图分离、消息驱动的架构",
        key_paths=("core", "widget", "runtime"),
    ),
    RefRepo(
        name="taffy",
        url="https://github.com/DioxusLabs/taffy.git",
        groups=("cpui",),
        focus="弹性盒布局计算",
        key_paths=("src",),
    ),
    RefRepo(
        name="xilem",
        url="https://github.com/linebender/xilem.git",
        groups=("cpui",),
        focus="组件组织与视图树设计",
        key_paths=("xilem", "xilem_core", "masonry"),
    ),
    # --- Rich-Text Editor Blueprint (docs/bluemap/richtext-editor.md) ---
    RefRepo(
        name="codemirror-view",
        url="https://github.com/codemirror/view.git",
        groups=("richtext-editor",),
        focus="视图层、位置映射与装饰层",
        key_paths=("src",),
    ),
    RefRepo(
        name="codemirror-state",
        url="https://github.com/codemirror/state.git",
        groups=("richtext-editor",),
        focus="状态模型、事务与位置映射",
        key_paths=("src",),
    ),
    RefRepo(
        name="lezer-markdown",
        url="https://github.com/lezer-parser/markdown.git",
        groups=("richtext-editor",),
        focus="增量 Markdown 解析",
        key_paths=("src",),
    ),
    RefRepo(
        name="zettlr",
        url="https://github.com/Zettlr/Zettlr.git",
        groups=("richtext-editor",),
        focus="以 Markdown 为数据源的实时预览写作体验",
        key_paths=("source/common/modules/markdown-editor",),
    ),
    RefRepo(
        name="marktext",
        url="https://github.com/marktext/marktext.git",
        groups=("richtext-editor",),
        focus="以 Markdown 为数据源的实时预览写作体验",
        key_paths=("packages/muya",),
    ),
    RefRepo(
        name="prosemirror-state",
        url="https://github.com/ProseMirror/prosemirror-state.git",
        groups=("richtext-editor",),
        focus="编辑器状态与事务模型",
        key_paths=("src",),
    ),
    RefRepo(
        name="prosemirror-inputrules",
        url="https://github.com/ProseMirror/prosemirror-inputrules.git",
        groups=("richtext-editor",),
        focus="ProseMirror 输入规则实现",
        key_paths=("src",),
    ),
    RefRepo(
        name="prosemirror-commands",
        url="https://github.com/ProseMirror/prosemirror-commands.git",
        groups=("richtext-editor",),
        focus="ProseMirror 编辑命令组织",
        key_paths=("src",),
    ),
    RefRepo(
        name="milkdown",
        url="https://github.com/Milkdown/milkdown.git",
        groups=("richtext-editor",),
        focus="输入规则与编辑命令组织",
        key_paths=("packages",),
    ),
    RefRepo(
        name="super_editor",
        url="https://github.com/Flutter-Bounty-Hunters/super_editor.git",
        groups=("richtext-editor",),
        focus="脱离浏览器的文档模型、布局与命令分层",
        key_paths=("super_editor/lib/src",),
    ),
    RefRepo(
        name="parley",
        url="https://github.com/linebender/parley.git",
        groups=("richtext-editor",),
        focus="富文本排版、字形成形与光标几何",
        key_paths=("parley/src",),
    ),
    RefRepo(
        name="cosmic-text",
        url="https://github.com/pop-os/cosmic-text.git",
        groups=("richtext-editor",),
        focus="富文本排版、字形成形与光标几何",
        key_paths=("src",),
    ),
    RefRepo(
        name="typst",
        url="https://github.com/typst/typst.git",
        groups=("richtext-editor",),
        focus="增量排版与数学排版",
        key_paths=("crates/typst-layout", "crates/typst-syntax"),
    ),
)


def run_git(args: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.setdefault("GIT_TERMINAL_PROMPT", "0")
    # Reference clones only need source code; disable Git LFS filters directly in git
    # so cloning works even without git-lfs installed and skips large binary downloads.
    env.setdefault("GIT_LFS_SKIP_SMUDGE", "1")
    return subprocess.run(
        [
            "git",
            "-c",
            "filter.lfs.smudge=",
            "-c",
            "filter.lfs.process=",
            "-c",
            "filter.lfs.required=false",
            *args,
        ],
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def format_git_error(result: subprocess.CompletedProcess[str], fallback: str) -> str:
    lines = [line.strip() for line in (result.stderr or result.stdout).splitlines() if line.strip()]
    return " | ".join(lines[-3:]) if lines else fallback


def is_git_repo(path: Path) -> bool:
    return path.is_dir() and (path / ".git").exists()


def head_short_sha(path: Path) -> str:
    if not is_git_repo(path):
        return "-"
    res = run_git(["rev-parse", "--short", "HEAD"], cwd=path)
    return res.stdout.strip() if res.returncode == 0 else "-"


def sync_repo(repo: RefRepo, base_dir: Path, update: bool, full: bool) -> tuple[str, str, str]:
    """Return (repo.name, status, detail) where status in {cloned, updated, skipped, failed}."""
    dest = base_dir / repo.name
    if is_git_repo(dest):
        if not update:
            return (repo.name, "skipped", f"already initialized ({head_short_sha(dest)})")
        fetch_args = ["fetch", "--prune", "origin"]
        if not full:
            fetch_args.insert(1, "--depth=1")
        fetch = run_git(fetch_args, cwd=dest)
        if fetch.returncode != 0:
            return (repo.name, "failed", format_git_error(fetch, "git fetch failed"))
        reset = run_git(["reset", "--hard", "FETCH_HEAD"], cwd=dest)
        if reset.returncode != 0:
            return (repo.name, "failed", format_git_error(reset, "git reset failed"))
        return (repo.name, "updated", head_short_sha(dest))

    # Clean up any leftover partial directory from an interrupted previous run.
    if dest.exists():
        shutil.rmtree(dest)

    staging = base_dir / f".{repo.name}.cloning"
    if staging.exists():
        shutil.rmtree(staging)

    clone_args = ["clone", "--single-branch"]
    if not full:
        clone_args.extend(["--depth", "1"])
    clone_args.extend([repo.url, str(staging)])

    result = run_git(clone_args)
    if result.returncode != 0:
        if staging.exists():
            shutil.rmtree(staging, ignore_errors=True)
        return (repo.name, "failed", format_git_error(result, "git clone failed"))

    staging.rename(dest)
    return (repo.name, "cloned", head_short_sha(dest))


def write_index(base_dir: Path) -> None:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    lines = [
        "# Blueprint Reference Repositories",
        "",
        "本目录由 `scripts/init-refs.py` 自动维护，已在 `.gitignore` 中忽略，仅作本地开发参考。",
        "",
        f"最后更新：{now}",
        "",
        "## 目录总览",
        "",
        "| 本地目录 | 蓝图 | 关注领域 | 重点子路径 | 状态 / Commit | 上游仓库 |",
        "|---|---|---|---|---|---|",
    ]
    for repo in REPOS:
        dest = base_dir / repo.name
        sha = head_short_sha(dest)
        status = f"`{sha}`" if sha != "-" else "未初始化"
        groups = ", ".join(repo.groups)
        subpaths = ", ".join(f"`{repo.name}/{p}`" for p in repo.key_paths) if repo.key_paths else f"`{repo.name}`"
        lines.append(
            f"| `{repo.name}/` | {groups} | {repo.focus} | {subpaths} | {status} | {repo.url.removesuffix('.git')} |"
        )
    lines.append("")
    (base_dir / "INDEX.md").write_text("\n".join(lines), encoding="utf-8")


def select_repos(group: str, only: list[str] | None) -> list[RefRepo]:
    selected = [r for r in REPOS if group == "all" or group in r.groups]
    if only:
        by_name = {r.name: r for r in REPOS}
        unknown = [name for name in only if name not in by_name]
        if unknown:
            valid = ", ".join(r.name for r in REPOS)
            raise SystemExit(f"未知项目：{', '.join(unknown)}\n可选项目：{valid}")
        only_set = set(only)
        selected = [r for r in selected if r.name in only_set]
    return selected


def print_list(repos: list[RefRepo], base_dir: Path) -> None:
    print(f"参考目录：{base_dir}")
    print(f"共 {len(repos)} 个参考项目：\n")
    for repo in repos:
        dest = base_dir / repo.name
        sha = head_short_sha(dest)
        state = f"ready ({sha})" if sha != "-" else "missing"
        groups = ",".join(repo.groups)
        print(f"  - {repo.name:<24} [{state:<16}] ({groups})  {repo.focus}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="初始化或更新 Minc 蓝图中的开源参考项目到本地 gitignore 目录。"
    )
    parser.add_argument(
        "--dir",
        type=Path,
        default=DEFAULT_DIR,
        help=f"参考源码存放目录（默认：{DEFAULT_DIR}）",
    )
    parser.add_argument(
        "--group",
        choices=("all", "cpui", "richtext-editor"),
        default="all",
        help="只初始化指定蓝图的参考项目（默认：all）",
    )
    parser.add_argument(
        "--only",
        nargs="+",
        metavar="NAME",
        help="只处理指定的项目名称（可用 --list 查看名称）",
    )
    parser.add_argument(
        "-u",
        "--update",
        action="store_true",
        help="已存在的仓库同步拉取最新代码（默认跳过已初始化的仓库）",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="拉取完整 git 历史（默认浅克隆 --depth 1）",
    )
    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=4,
        help="并发拉取数量（默认：4）",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="仅列出所有参考项目及本地初始化状态，不执行网络操作",
    )
    args = parser.parse_args()

    if args.jobs < 1:
        parser.error("--jobs must be >= 1")

    base_dir = args.dir.expanduser().resolve()
    repos = select_repos(args.group, args.only)

    if args.list:
        print_list(repos, base_dir)
        return 0

    if shutil.which("git") is None:
        print("错误：未找到 git 命令，请先安装 git。", file=sys.stderr)
        return 1

    base_dir.mkdir(parents=True, exist_ok=True)
    mode = "初始化并更新" if args.update else "初始化（已存在则跳过）"
    print(f"[init-refs] 目录：{base_dir}")
    print(f"[init-refs] 模式：{mode}，项目数：{len(repos)}，并发：{args.jobs}\n", flush=True)

    counts = {"cloned": 0, "updated": 0, "skipped": 0, "failed": 0}
    failures: list[tuple[str, str]] = []

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        future_map = {
            pool.submit(sync_repo, repo, base_dir, args.update, args.full): repo
            for repo in repos
        }
        for future in as_completed(future_map):
            name, status, detail = future.result()
            counts[status] += 1
            if status == "failed":
                failures.append((name, detail))
            tag = {
                "cloned": "CLONED ",
                "updated": "UPDATED",
                "skipped": "SKIPPED",
                "failed": "FAILED ",
            }[status]
            print(f"  [{tag}] {name:<24} {detail}", flush=True)

    write_index(base_dir)

    print(
        f"\n[init-refs] 完成：新增 {counts['cloned']}，"
        f"更新 {counts['updated']}，跳过 {counts['skipped']}，失败 {counts['failed']}。"
    )
    print(f"[init-refs] 索引已生成：{base_dir / 'INDEX.md'}")

    if failures:
        print("\n失败详情（可直接重跑脚本续传）：", file=sys.stderr)
        for name, detail in failures:
            print(f"  - {name}: {detail}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\n[init-refs] 已中断，重新运行脚本即可继续初始化剩余项目。", file=sys.stderr)
        sys.exit(130)
