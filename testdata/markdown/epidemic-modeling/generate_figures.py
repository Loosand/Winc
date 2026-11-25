#!/usr/bin/env python3
"""生成《传染病动力学入门》一文使用的插图。

只依赖 Python 标准库，输出确定、可重复：
- images/sir-curves.svg              SIR 模型数值解（RK4 积分）
- images/herd-immunity-threshold.svg 群体免疫阈值曲线与常见疾病的 R0 区间
- images/attack-rate-heatmap.png     不同 R0 与疫苗覆盖率下的最终感染比例

运行：python3 generate_figures.py
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
IMAGES = HERE / "images"

# SIR 参数：与正文「4.1 一个数值例子」一致。
BETA = 0.3  # 传播率（每天）
GAMMA = 0.1  # 康复率（每天），平均感染期 10 天
I0 = 1e-4  # 初始感染比例
DAYS = 160
DT = 0.1


def sir_rhs(s: float, i: float) -> tuple[float, float, float]:
    new_inf = BETA * s * i
    return -new_inf, new_inf - GAMMA * i, GAMMA * i


def integrate() -> list[tuple[float, float, float, float]]:
    s, i, r = 1.0 - I0, I0, 0.0
    out = [(0.0, s, i, r)]
    steps = int(DAYS / DT)
    for n in range(1, steps + 1):
        k1 = sir_rhs(s, i)
        k2 = sir_rhs(s + DT * k1[0] / 2, i + DT * k1[1] / 2)
        k3 = sir_rhs(s + DT * k2[0] / 2, i + DT * k2[1] / 2)
        k4 = sir_rhs(s + DT * k3[0], i + DT * k3[1])
        s += DT * (k1[0] + 2 * k2[0] + 2 * k3[0] + k4[0]) / 6
        i += DT * (k1[1] + 2 * k2[1] + 2 * k3[1] + k4[1]) / 6
        r += DT * (k1[2] + 2 * k2[2] + 2 * k3[2] + k4[2]) / 6
        out.append((n * DT, s, i, r))
    return out


def final_size(r0: float) -> float:
    """解 z = 1 - exp(-R0 z) 的正根；R0 <= 1 时为 0。"""
    if r0 <= 1.0:
        return 0.0
    z = 0.5
    for _ in range(200):
        z = 1.0 - math.exp(-r0 * z)
    return z


# ---------------------------------------------------------------- SVG 工具


def svg_doc(width: int, height: int, body: list[str]) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" font-family="Helvetica, Arial, sans-serif">\n'
        f'<rect width="{width}" height="{height}" fill="#ffffff"/>\n'
        + "\n".join(body)
        + "\n</svg>\n"
    )


def axes(
    body: list[str],
    box: tuple[float, float, float, float],
    xticks: list[tuple[float, str]],
    yticks: list[tuple[float, str]],
    xlabel: str,
    ylabel: str,
) -> None:
    left, top, w, h = box
    for y, label in yticks:
        py = top + h - y * h
        body.append(
            f'<line x1="{left}" y1="{py:.1f}" x2="{left + w}" y2="{py:.1f}" '
            f'stroke="#e5e7eb" stroke-width="1"/>'
        )
        body.append(
            f'<text x="{left - 8}" y="{py + 4:.1f}" font-size="12" fill="#4b5563" '
            f'text-anchor="end">{label}</text>'
        )
    for x, label in xticks:
        px = left + x * w
        body.append(
            f'<line x1="{px:.1f}" y1="{top + h}" x2="{px:.1f}" y2="{top + h + 5}" '
            f'stroke="#6b7280" stroke-width="1"/>'
        )
        body.append(
            f'<text x="{px:.1f}" y="{top + h + 20}" font-size="12" fill="#4b5563" '
            f'text-anchor="middle">{label}</text>'
        )
    body.append(
        f'<line x1="{left}" y1="{top + h}" x2="{left + w}" y2="{top + h}" '
        f'stroke="#6b7280" stroke-width="1.2"/>'
    )
    body.append(
        f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + h}" '
        f'stroke="#6b7280" stroke-width="1.2"/>'
    )
    body.append(
        f'<text x="{left + w / 2}" y="{top + h + 42}" font-size="13" fill="#111827" '
        f'text-anchor="middle">{xlabel}</text>'
    )
    body.append(
        f'<text x="{left - 46}" y="{top + h / 2}" font-size="13" fill="#111827" '
        f'text-anchor="middle" transform="rotate(-90 {left - 46} {top + h / 2})">{ylabel}</text>'
    )


def sir_svg(series: list[tuple[float, float, float, float]]) -> tuple[str, dict[str, float]]:
    width, height = 720, 420
    box = (70.0, 30.0, 600.0, 310.0)
    left, top, w, h = box
    body: list[str] = []
    axes(
        body,
        box,
        [(d / DAYS, str(d)) for d in range(0, DAYS + 1, 20)],
        [(v / 10, f"{v / 10:.1f}") for v in range(0, 11, 2)],
        "时间（天）",
        "人群比例",
    )

    peak = max(series, key=lambda row: row[2])
    r0 = BETA / GAMMA
    # 峰值时刻 S = 1/R0 的参考线。
    sy = top + h - (1 / r0) * h
    body.append(
        f'<line x1="{left}" y1="{sy:.1f}" x2="{left + w}" y2="{sy:.1f}" '
        f'stroke="#9ca3af" stroke-width="1" stroke-dasharray="6 4"/>'
    )
    body.append(
        f'<text x="{left + w - 4}" y="{sy - 6:.1f}" font-size="12" fill="#6b7280" '
        f'text-anchor="end">S = 1/R₀</text>'
    )
    px = left + peak[0] / DAYS * w
    body.append(
        f'<line x1="{px:.1f}" y1="{top}" x2="{px:.1f}" y2="{top + h}" '
        f'stroke="#9ca3af" stroke-width="1" stroke-dasharray="6 4"/>'
    )
    body.append(
        f'<text x="{px + 6:.1f}" y="{top + 14}" font-size="12" fill="#6b7280">'
        f"感染高峰：第 {peak[0]:.0f} 天</text>"
    )

    colors = {1: "#2563eb", 2: "#dc2626", 3: "#16a34a"}
    names = {1: "易感者 S", 2: "感染者 I", 3: "康复者 R"}
    for k in (1, 2, 3):
        pts = " ".join(
            f"{left + row[0] / DAYS * w:.1f},{top + h - row[k] * h:.1f}"
            for row in series[::10]
        )
        body.append(
            f'<polyline points="{pts}" fill="none" stroke="{colors[k]}" stroke-width="2.4"/>'
        )
    for n, k in enumerate((1, 2, 3)):
        lx, ly = left + w - 150, top + 60 + n * 22
        body.append(
            f'<line x1="{lx}" y1="{ly}" x2="{lx + 26}" y2="{ly}" '
            f'stroke="{colors[k]}" stroke-width="3"/>'
        )
        body.append(
            f'<text x="{lx + 34}" y="{ly + 4}" font-size="13" fill="#111827">{names[k]}</text>'
        )

    stats = {
        "peak_day": peak[0],
        "peak_i": peak[2],
        "final_r": series[-1][3],
        "final_size_eq": final_size(r0),
    }
    return svg_doc(width, height, body), stats


# 常见疾病的 R0 代表性估计区间：与正文表 2 一致。
DISEASES = [
    ("季节性流感", 1.2, 1.4),
    ("COVID-19（早期毒株）", 2.0, 3.0),
    ("天花", 3.5, 6.0),
    ("脊髓灰质炎", 5.0, 7.0),
    ("风疹", 6.0, 7.0),
    ("水痘", 10.0, 12.0),
    ("麻疹", 12.0, 18.0),
]


def threshold_svg() -> str:
    width, height = 720, 420
    box = (70.0, 30.0, 600.0, 310.0)
    left, top, w, h = box
    r_max = 20.0
    body: list[str] = []
    axes(
        body,
        box,
        [(r / r_max, str(r)) for r in range(0, 21, 2)],
        [(v / 10, f"{v * 10}%") for v in range(0, 11, 2)],
        "基本再生数 R₀",
        "群体免疫阈值",
    )
    pts = []
    r = 1.0
    while r <= r_max + 1e-9:
        pts.append(f"{left + r / r_max * w:.1f},{top + h - (1 - 1 / r) * h:.1f}")
        r += 0.1
    body.append(
        f'<polyline points="{" ".join(pts)}" fill="none" stroke="#7c3aed" stroke-width="2.4"/>'
    )
    for n, (name, lo, hi) in enumerate(DISEASES):
        x0, x1 = left + lo / r_max * w, left + hi / r_max * w
        mid = (lo + hi) / 2
        y = top + h - (1 - 1 / mid) * h
        body.append(
            f'<line x1="{x0:.1f}" y1="{y:.1f}" x2="{x1:.1f}" y2="{y:.1f}" '
            f'stroke="#f59e0b" stroke-width="5" stroke-linecap="round" opacity="0.85"/>'
        )
        dy = 18 if n % 2 == 0 else -10
        body.append(
            f'<text x="{(x0 + x1) / 2:.1f}" y="{y + dy:.1f}" font-size="11" fill="#374151" '
            f'text-anchor="middle">{name}</text>'
        )
    body.append(
        f'<text x="{left + 12}" y="{top + 16}" font-size="13" fill="#7c3aed">'
        f"H = 1 − 1/R₀</text>"
    )
    return svg_doc(width, height, body)


# ---------------------------------------------------------------- PNG 工具

VIRIDIS = [
    (0.00, (68, 1, 84)),
    (0.25, (59, 82, 139)),
    (0.50, (33, 145, 140)),
    (0.75, (94, 201, 98)),
    (1.00, (253, 231, 37)),
]


def colormap(t: float) -> tuple[int, int, int]:
    t = min(max(t, 0.0), 1.0)
    for (t0, c0), (t1, c1) in zip(VIRIDIS, VIRIDIS[1:]):
        if t <= t1:
            k = (t - t0) / (t1 - t0)
            return tuple(round(a + (b - a) * k) for a, b in zip(c0, c1))  # type: ignore[return-value]
    return VIRIDIS[-1][1]


def write_png(path: Path, width: int, height: int, rows: list[bytes]) -> None:
    raw = b"".join(b"\x00" + row for row in rows)

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


def heatmap_png(path: Path) -> None:
    """横轴 R0 ∈ [1, 8]，纵轴疫苗覆盖率 p ∈ [0, 95%]（自下而上），颜色为最终感染比例。

    白色曲线为群体免疫阈值 p = 1 − 1/R0：曲线上方不会发生大规模流行。
    """
    width, height = 640, 360
    rows = []
    for py in range(height):
        p = 0.95 * (1 - py / (height - 1))
        row = bytearray()
        for px in range(width):
            r0 = 1.0 + 7.0 * px / (width - 1)
            attack = (1 - p) * final_size(r0 * (1 - p))
            threshold = 1 - 1 / r0
            ty = (1 - threshold / 0.95) * (height - 1)
            if abs(py - ty) < 1.2:
                row += bytes((255, 255, 255))
            else:
                row += bytes(colormap(attack))
        rows.append(bytes(row))
    write_png(path, width, height, rows)


def main() -> None:
    IMAGES.mkdir(parents=True, exist_ok=True)
    svg, stats = sir_svg(integrate())
    (IMAGES / "sir-curves.svg").write_text(svg, encoding="utf-8")
    (IMAGES / "herd-immunity-threshold.svg").write_text(threshold_svg(), encoding="utf-8")
    heatmap_png(IMAGES / "attack-rate-heatmap.png")
    for key, value in stats.items():
        print(f"{key} = {value:.4f}")


if __name__ == "__main__":
    main()
