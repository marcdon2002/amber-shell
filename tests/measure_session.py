#!/usr/bin/env python3
"""Measure only the release PTY relay; separate from zsh, UI and Codex."""
import argparse
import hashlib
import json
from pathlib import Path
import time

import measure_terminal as measure
import session_e2e as capture
import zle_binary_e2e as harness


def idle(session, seconds):
    session.drain(.1)
    start = len(session.output)
    samples = [measure.sample(session.pid)]
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        session.drain(.2)
        samples.append(measure.sample(session.pid))
    return measure.summarize(samples, len(session.output) - start)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=harness.ROOT/'target/release/amber-shell')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--idle-seconds', type=float, default=8)
    args = parser.parse_args()
    harness.BINARY = args.binary.resolve()
    session = harness.ShellSession(capture_session=True)
    try:
        before = idle(session, args.idle_seconds)
        for number in range(5):
            command = f"printf 'SYNTHETIC_OUTPUT_{number}\\n'"
            session.send(command+'\r')
            session.until(lambda: capture.snapshot(session) and
                          capture.snapshot(session)[-1]['command'] == command,
                          label='completed synthetic capture')
        records = capture.snapshot(session)
        after = idle(session, args.idle_seconds)
        result = dict(
            binary=str(harness.BINARY),
            sha256=hashlib.sha256(harness.BINARY.read_bytes()).hexdigest(),
            scope='Actual macOS PTY relay only; excludes zsh, UI, Codex and Apple Terminal',
            idle_before_commands=before, idle_after_commands=after,
            records=len(records), snapshot_bytes=len(json.dumps(records).encode()))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2)+'\n')
        print(json.dumps(result, indent=2))
    finally:
        session.close()


if __name__ == '__main__':
    main()
