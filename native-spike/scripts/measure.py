#!/usr/bin/env python3
"""Measures the running Sirus the same way every time, so a change can be compared with the last run.

    scripts/measure.py idle --seconds 60        # a label, then how long to sample
    scripts/measure.py working --budget 80      # fails when median CPU goes over the budget

Prints the reading and appends it to target/perf/readings.jsonl, with the difference from the last
reading of that label. Sampling reads the process table once a second, so it costs next to nothing,
and nothing runs unless this script is started. A reading is only comparable with one taken in the
same state: no agent working for "idle", one agent replying for "working", the window covered for
"hidden".
"""
import argparse, json, pathlib, re, statistics, subprocess, sys, time

APP = "Sirus.app/Contents/MacOS/sirus-native-spike"
HOST = "Sirus.app/Contents/MacOS/sirus-host"
LOG = pathlib.Path(__file__).resolve().parent.parent / "target" / "perf" / "readings.jsonl"


def table():
    out = subprocess.run(["ps", "-axo", "pid=,pcpu=,command="], capture_output=True, text=True).stdout
    for line in out.splitlines():
        pid, cpu, command = line.split(None, 2)
        yield int(pid), float(cpu), command


def footprint_mb(pid):
    out = subprocess.run(["footprint", str(pid)], capture_output=True, text=True).stdout
    found = re.search(r"Footprint:\s*([\d.]+)\s*(KB|MB|GB)", out)
    if not found:
        return None
    return round(float(found[1]) * {"KB": 1 / 1024, "MB": 1, "GB": 1024}[found[2]], 1)


def read(label, seconds):
    app_cpu, host_cpu = [], []
    for _ in range(seconds):
        rows = list(table())
        app_cpu.append(sum(cpu for _, cpu, command in rows if APP in command))
        host_cpu.append(sum(cpu for _, cpu, command in rows if HOST in command))
        time.sleep(1)
    rows = list(table())
    apps = [pid for pid, _, command in rows if APP in command]
    if not apps:
        sys.exit("Sirus is not running")
    hosts = [(pid, command) for pid, _, command in rows if HOST in command]
    children = [pid for pid, command in hosts if "--mcp-" in command]
    servers = [pid for pid, command in hosts if "--mcp-" not in command]
    sum_of = lambda pids: round(sum(filter(None, map(footprint_mb, pids))), 1)
    return {
        "label": label,
        "at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "seconds": seconds,
        "cpu_median": round(statistics.median(app_cpu), 1),
        "cpu_p90": round(sorted(app_cpu)[int(len(app_cpu) * 0.9)], 1),
        "host_cpu_median": round(statistics.median(host_cpu), 1),
        "window_mb": sum_of(apps),
        "host_mb": sum_of(servers),
        "mcp_children": len(children),
        "mcp_mb": sum_of(children),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("label")
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--budget", type=float, help="fail when median CPU is over this")
    args = parser.parse_args()
    reading = read(args.label, args.seconds)
    LOG.parent.mkdir(parents=True, exist_ok=True)
    previous = None
    if LOG.exists():
        for line in LOG.read_text().splitlines():
            old = json.loads(line)
            if old["label"] == args.label:
                previous = old
    with LOG.open("a") as log:
        log.write(json.dumps(reading) + "\n")
    for key, value in reading.items():
        change = ""
        if previous and isinstance(value, (int, float)) and key in previous:
            change = f"  ({value - previous[key]:+.1f})"
        print(f"{key:16} {value}{change}")
    if args.budget is not None and reading["cpu_median"] > args.budget:
        sys.exit(f"median CPU {reading['cpu_median']} is over the budget of {args.budget}")


if __name__ == "__main__":
    main()
