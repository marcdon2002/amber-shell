#!/usr/bin/env python3
"""Read-only macOS process measurements of the real binary in isolated PTYs.

No native terminal automation, real provider calls, or product settings writes.
Startup is first-observed input readiness with ~30 ms harness granularity.
RSS/footprint peaks are sampled, not a kernel lifetime maximum. Wakeups are
macOS proc_pid_rusage counters, not claimed timer or energy measurements.
"""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import statistics
import time

import terminal_e2e as harness


class Usage(ctypes.Structure):
    _fields_=[('uuid',ctypes.c_uint8*16)]+[(name,ctypes.c_uint64) for name in (
        'user_ticks','system_ticks','package_idle_wakeups','interrupt_wakeups','pageins',
        'wired_bytes','rss_bytes','footprint_bytes','start_abstime','exit_abstime')]


class TaskInfo(ctypes.Structure):
    _fields_=[(name,ctypes.c_uint64) for name in (
        'virtual_bytes','rss_bytes','user_ticks','system_ticks','threads_user_ticks','threads_system_ticks')]+[
        (name,ctypes.c_int32) for name in ('policy','faults','pageins','cow_faults',
        'messages_sent','messages_received','syscalls_mach','syscalls_unix','context_switches',
        'thread_count','running_threads','priority')]


lib=ctypes.CDLL('/usr/lib/libproc.dylib',use_errno=True)
lib.proc_pid_rusage.argtypes=[ctypes.c_int,ctypes.c_int,ctypes.c_void_p]
lib.proc_pidinfo.argtypes=[ctypes.c_int,ctypes.c_int,ctypes.c_uint64,ctypes.c_void_p,ctypes.c_int]

class Timebase(ctypes.Structure):
    _fields_=[('numer',ctypes.c_uint32),('denom',ctypes.c_uint32)]

timebase=Timebase()
ctypes.CDLL('/usr/lib/libSystem.B.dylib').mach_timebase_info(ctypes.byref(timebase))


def sample(pid):
    usage=Usage()
    task=TaskInfo()
    if lib.proc_pid_rusage(pid,0,ctypes.byref(usage)) != 0:
        raise OSError(ctypes.get_errno(),'proc_pid_rusage')
    if lib.proc_pidinfo(pid,4,0,ctypes.byref(task),ctypes.sizeof(task)) != ctypes.sizeof(task):
        raise OSError(ctypes.get_errno(),'proc_pidinfo TASKINFO')
    # libproc reports CPU counters in Mach ticks on this ARM64 host. Convert
    # with the actual timebase (125/3 here), verified against process_time_ns.
    return dict(wall=time.monotonic(),cpu_ns=(usage.user_ticks+usage.system_ticks)*timebase.numer/timebase.denom,
                rss_bytes=usage.rss_bytes,footprint_bytes=usage.footprint_bytes,
                interrupt_wakeups=usage.interrupt_wakeups,package_idle_wakeups=usage.package_idle_wakeups,
                context_switches=task.context_switches,thread_count=task.thread_count)


def summarize(samples,output_bytes):
    first,last=samples[0],samples[-1]
    wall=last['wall']-first['wall']
    return dict(wall_seconds=round(wall,3),cpu_seconds=round((last['cpu_ns']-first['cpu_ns'])/1e9,6),
                cpu_percent_one_core=round((last['cpu_ns']-first['cpu_ns'])/1e9/wall*100,3),
                rss_mib_min=round(min(s['rss_bytes'] for s in samples)/1048576,3),
                rss_mib_max=round(max(s['rss_bytes'] for s in samples)/1048576,3),
                footprint_mib_max=round(max(s['footprint_bytes'] for s in samples)/1048576,3),
                interrupt_wakeups_per_second=round((last['interrupt_wakeups']-first['interrupt_wakeups'])/wall,3),
                package_idle_wakeups_per_second=round((last['package_idle_wakeups']-first['package_idle_wakeups'])/wall,3),
                context_switches_per_second=round((last['context_switches']-first['context_switches'])/wall,3),
                threads_max=max(s['thread_count'] for s in samples),pty_output_bytes=output_bytes)


def idle(session,seconds):
    session.read(.1)
    output_start=len(session.output)
    snapshots=[sample(session.proc.pid)]
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        session.read(.2)
        snapshots.append(sample(session.proc.pid))
    return summarize(snapshots,len(session.output)-output_start)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=harness.ROOT/'target/release/amber-shell')
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--cwd',type=Path,help='Optional project directory for startup/context measurements')
    parser.add_argument('--question',default='Synthetische Messfrage',help='Synthetic question prefix; use a project question to measure lazy discovery')
    parser.add_argument('--idle-seconds',type=float,default=8)
    parser.add_argument('--launches',type=int,default=10)
    parser.add_argument('--responses',type=int,default=5)
    args=parser.parse_args()
    harness.BIN=args.binary.resolve()
    launch_ms=[]
    for _ in range(args.launches):
        started=time.monotonic()
        session=harness.Session(review=False,cwd=args.cwd)
        launch_ms.append((time.monotonic()-started)*1000)
        try:
            session.send(b'\x1b');session.wait()
        finally:session.close()
    session=harness.Session(review=False,cwd=args.cwd)
    try:
        initial=idle(session,args.idle_seconds)
        snapshots=[sample(session.proc.pid)]
        output_start=len(session.output)
        response_ms=[]
        for index in range(args.responses):
            started=time.monotonic()
            session.send(f'{args.question} {index}\r')
            saw_active=False
            deadline=time.monotonic()+10
            while time.monotonic()<deadline:
                session.read(.05)
                snapshots.append(sample(session.proc.pid))
                saw_active |= 'Antwort läuft' in session.text()
                if saw_active and 'Fertig' in session.status_text():break
            else:raise AssertionError('simulated response did not finish')
            response_ms.append(round((time.monotonic()-started)*1000,3))
        active=summarize(snapshots,len(session.output)-output_start)
        completed=idle(session,args.idle_seconds)
        session.send(b'\x1b');session.wait()
    finally:session.close()
    result=dict(question=args.question,cwd=str(args.cwd.resolve()) if args.cwd else "isolated temporary directory",binary=str(harness.BIN),sha256=hashlib.sha256(harness.BIN.read_bytes()).hexdigest(),
                mach_timebase=dict(numer=timebase.numer,denom=timebase.denom),
                scope='Actual macOS binary in isolated PTY, simulation only; no Apple Terminal GUI measurement',
                startup=dict(count=len(launch_ms),min_ms=round(min(launch_ms),3),
                             median_ms=round(statistics.median(launch_ms),3),max_ms=round(max(launch_ms),3),
                             samples_ms=[round(value,3) for value in launch_ms]),
                idle_before_request=initial,active_simulation=active,simulation_response_ms=response_ms,
                idle_after_responses=completed)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
