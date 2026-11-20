"""Диаграмма statistics.txt: python plot_statistics.py statistics.txt.

Зависимость: python -m pip install matplotlib
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Interval:
    lane: str
    name: str
    start: float
    duration: float
    kind: str

    @property
    def end(self) -> float:
        return self.start + self.duration


def read_statistics(
        path: Path,
        worker_order: str = "start-duration",
        writer_order: str = "start-duration",
) -> tuple[list[Interval], float | None]:
    raw = path.read_bytes()
    encoding = "utf-16" if raw.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8-sig"
    text = raw.decode(encoding)
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    intervals: list[Interval] = []
    worker: str | None = None
    block: list[str] = []
    writing = False
    total: float | None = None

    def flush() -> None:
        if worker is not None or writing:
            lane = worker if worker is not None else "Запись артефактов"
            kind = "worker" if worker is not None else "writer"
            order = worker_order if worker is not None else writer_order
            intervals.extend(_parse_records("\n".join(block), lane, kind, order))
        block.clear()

    for line in text.splitlines():
        marker = re.search(r"ПОТОК\s*#(\d+)\)", line)
        if marker or "=== Запись файлов в целевой архив" in line or "===>" in line:
            flush()
            worker = f"Поток #{marker[1]}" if marker else None
            writing = "=== Запись файлов в целевой архив" in line
            if "===>" in line:
                stage = _parse_stage(line)
                if stage is not None:
                    intervals.append(stage)
                if "TOTAL TIME" in line:
                    match = _TIME_RE.search(line)
                    if match:
                        total = _milliseconds(match[0])
        else:
            block.append(line)
    flush()
    if not any(item.kind in {"worker", "writer"} for item in intervals):
        raise ValueError("Не найдены записи работников или записи артефактов.")
    return intervals, total


def plot_statistics(
        intervals: list[Interval],
        output: Path,
        total: float | None = None,
        show: bool = True,
) -> None:
    from bisect import bisect_right
    from collections import defaultdict
    from math import ceil, log10

    import matplotlib.pyplot as plt
    from matplotlib.artist import Artist
    from matplotlib.backend_bases import RendererBase
    from matplotlib.text import Text
    from matplotlib.ticker import MultipleLocator

    groups: dict[str, list[Interval]] = defaultdict(list)
    for item in intervals:
        groups[item.lane].append(item)
    workers = sorted((lane for lane in groups if lane.startswith("Поток #")),
                     key=lambda lane: int(lane.split("#")[1]))
    order = ["Чтение директорий", "Группировка", *workers, "Запись артефактов", "Сброс на диск", "Footer"]
    lanes = [lane for lane in order if lane in groups]
    starts: dict[str, list[float]] = {}
    ends: dict[str, list[float]] = {}
    file_colors: dict[str, Any] = {}
    palette = plt.get_cmap("tab20")
    colors = {"walk": "#8b5cf6", "group": "#eab308", "sync": "#dc2626", "footer": "#db2777"}
    end = max(max(item.end for item in intervals), total or 0.0)
    fig, ax = plt.subplots(figsize=(17, max(4, len(lanes) * 0.52 + 1.5)), constrained_layout=True)
    for y, lane in enumerate(lanes):
        items = groups[lane]
        items.sort(key=lambda item: item.start)
        starts[lane] = [item.start for item in items]
        ends[lane] = []
        ranges = []
        faces = []
        for item in items:
            ends[lane].append(max(item.end, ends[lane][-1] if ends[lane] else 0))
            ranges.append((item.start, item.duration))
            if item.kind in colors:
                faces.append(colors[item.kind])
            else:
                if item.name not in file_colors:
                    file_colors[item.name] = palette(len(file_colors) % 20)
                faces.append(file_colors[item.name])
        ax.broken_barh(ranges, (y - 0.34, 0.68), facecolors=faces, edgecolors="none")

    class Labels(Artist):
        def draw(self, renderer: RendererBase) -> None:
            left, right = ax.get_xlim()
            scale = ax.bbox.width / (right - left)
            text = Text(ha="center", va="center", fontsize=8, parse_math=False)
            text.set_figure(fig)
            text.set_transform(ax.transData)
            padding = renderer.points_to_pixels(2)
            for y, lane in enumerate(lanes):
                items = groups[lane]
                if items[0].kind not in {"worker", "writer"}:
                    continue
                height = abs(ax.transData.transform((0, 0.68))[1] - ax.transData.transform((0, 0))[1])
                for i in range(bisect_right(ends[lane], left), bisect_right(starts[lane], right)):
                    item = items[i]
                    a, b = max(left, item.start), min(right, item.end)
                    width = (b - a) * scale
                    if width <= 2 * padding:
                        continue
                    text.set_text(item.name.rsplit("/", 1)[-1])
                    text.set_position(((a + b) / 2, y))
                    box = text.get_window_extent(renderer)
                    if box.width + 2 * padding <= width and box.height <= height:
                        text.draw(renderer)

    labels = Labels()
    labels.set_zorder(3)
    ax.add_artist(labels)
    if total is not None:
        ax.axvline(total, color="#111827", linestyle="--", linewidth=1)
    ax.set(yticks=range(len(lanes)), yticklabels=lanes, ylim=(len(lanes) - 0.4, -0.7),
           xlabel="Время от начала программы, мс", title="Обработка файлов и запись архива")

    def update_grid(_: Any) -> None:
        left, right = ax.get_xlim()
        step = 10 ** max(0, ceil(log10(max(abs(right - left) / 250, 1))))
        ax.xaxis.set_major_locator(MultipleLocator(10 * step))
        ax.xaxis.set_minor_locator(MultipleLocator(step))

    ax.callbacks.connect("xlim_changed", update_grid)
    ax.set_xlim(0, max(1, end * 1.01))
    ax.grid(axis="x", which="minor", alpha=0.18)
    ax.grid(axis="x", which="major", alpha=0.45)
    ax.set_axisbelow(True)

    def coordinates(x: float, y: float) -> str:
        row = round(y)
        if 0 <= row < len(lanes) and abs(y - row) <= 0.34:
            lane = lanes[row]
            index = bisect_right(starts[lane], x) - 1
            if index >= 0:
                item = groups[lane][index]
                if x <= item.end:
                    return f"{item.name}   {item.start:.4f}–{item.end:.4f} мс   Δ {item.duration:.4f} мс"
        return f"{x:.4f} мс"

    ax.format_coord = coordinates
    output.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(output, dpi=360)
    if show:
        plt.show()
    plt.close(fig)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("input", nargs="?", type=Path, default=Path("statistics.txt"))
    parser.add_argument("-o", "--output", type=Path, help="PNG, SVG или PDF; по умолчанию statistics.png рядом с отчётом")
    parser.add_argument("--no-show", action="store_true", help="Только сохранить, не открывать окно")
    for role in ("worker", "writer"):
        parser.add_argument(f"--{role}-order", choices=("start-duration", "duration-start"), default="start-duration")
    args = parser.parse_args()
    try:
        intervals, total = read_statistics(args.input, args.worker_order, args.writer_order)
        _check_intervals(intervals)
        output = args.output or args.input.with_suffix(".png")
        if output.resolve() == args.input.resolve():
            raise ValueError("Входной и выходной файлы должны различаться.")
        if args.no_show:
            import matplotlib
            matplotlib.use("Agg")
        plot_statistics(intervals, output, total, show=not args.no_show)
        print(f"Сохранено: {output}\nИнтервалов: {len(intervals)}")
        return 0
    except (OSError, ValueError, ImportError) as error:
        print(f"Ошибка: {error}", file=sys.stderr)
        return 1


_TIME = r"\d+(?:[.,]\d+)?\s*(?:ns|µs|μs|us|ms|s)"
_TIME_RE = re.compile(_TIME)
_RECORD_RE = re.compile(
    r'relative_path:\s*"(?P<path>(?:\\.|[^"\\])*)",.*?'
    r'\n\s*},\s*\n\s*(?P<first>' + _TIME + r'),\s*\n\s*(?P<second>' + _TIME + r'),\s*\n\s*\),',
    re.DOTALL,
    )


def _milliseconds(value: str) -> float:
    match = re.fullmatch(r"(\d+(?:[.,]\d+)?)\s*(ns|µs|μs|us|ms|s)", value.strip())
    if match is None:
        raise ValueError(f"Непонятное время: {value!r}")
    factors = {"ns": 0.000001, "µs": 0.001, "μs": 0.001, "us": 0.001, "ms": 1.0, "s": 1000.0}
    return float(match[1].replace(",", ".")) * factors[match[2]]


def _parse_records(text: str, lane: str, kind: str, order: str) -> list[Interval]:
    result: list[Interval] = []
    for match in _RECORD_RE.finditer(text):
        first, second = _milliseconds(match["first"]), _milliseconds(match["second"])
        start, duration = (first, second) if order == "start-duration" else (second, first)
        name = json.loads('"' + match["path"] + '"')
        result.append(Interval(lane, name, start, duration, kind))
    expected = len(re.findall(r"relative_path:", text))
    if len(result) != expected:
        raise ValueError(f"{lane}: найдено {expected} файлов, распознано {len(result)}. Проверьте формат отчёта.")
    return result


def _parse_stage(line: str) -> Interval | None:
    duration = re.search(r"Длительность:\s*(" + _TIME + ")", line)
    start = re.search(r"Начало[:\s]+(" + _TIME + ")", line)
    if "Чтение директорий" in line:
        lane, kind = "Чтение директорий", "walk"
        start_ms = _milliseconds(start[1]) if start else 0.0
    elif "группировки" in line.lower():
        lane, kind = "Группировка", "group"
        start = re.search(r"Начало группировки:\s*(" + _TIME + ")", line)
        start_ms = _milliseconds(start[1]) if start else None
    elif "Сброс всех изменений" in line:
        lane, kind = "Сброс на диск", "sync"
        start_ms = _milliseconds(start[1]) if start else None
    elif "footer" in line.lower():
        lane, kind = "Footer", "footer"
        start_ms = _milliseconds(start[1]) if start else None
    else:
        return None
    if duration is None or start_ms is None:
        print(f"Предупреждение: пропущен этап без явного начала/длительности: {line}", file=sys.stderr)
        return None
    return Interval(lane, lane, start_ms, _milliseconds(duration[1]), kind)


def _check_intervals(intervals: list[Interval]) -> None:
    for lane in dict.fromkeys(item.lane for item in intervals):
        previous: Interval | None = None
        for item in sorted((item for item in intervals if item.lane == lane), key=lambda item: item.start):
            if previous is not None and item.start < previous.end - 0.000001:
                print(f"Предупреждение: пересечение в {lane}: {previous.name} / {item.name}. Проверьте порядок времён.", file=sys.stderr)
            if previous is None or item.end > previous.end:
                previous = item


if __name__ == "__main__":
    raise SystemExit(main())
