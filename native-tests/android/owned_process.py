"""Linux CI fixture cleanup: bind signals to a captured child birth via pidfd."""
import argparse
import os
from pathlib import Path
import select
import signal


def identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[1]), int(fields[19])  # parent PID, kernel start ticks


def birth(pid, parent):
    try:
        actual_parent, started = identity(pid)
    except FileNotFoundError:
        return "exited"
    if actual_parent != parent:
        raise RuntimeError("Refusing to capture a process outside the launching shell")
    return str(started)


def stop(pid, started):
    if started == "exited":
        return
    expected = int(started)
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        poll = select.poll()
        poll.register(fd, select.POLLIN)
        if poll.poll(0):
            return
        # Open first, then compare birth. The pidfd keeps subsequent signals
        # bound to this process even if the numeric PID is later reused.
        try:
            _, actual = identity(pid)
        except FileNotFoundError:
            if poll.poll(0):
                return
            raise
        if actual != expected:
            raise RuntimeError("Refusing to signal a different process birth")
        for kind, milliseconds in ((signal.SIGTERM, 3000), (signal.SIGKILL, 2000)):
            try:
                signal.pidfd_send_signal(fd, kind)
            except ProcessLookupError:
                return
            if poll.poll(milliseconds):
                return
        raise TimeoutError("Owned process did not exit within the cleanup bound")
    finally:
        os.close(fd)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("birth", "stop"))
    parser.add_argument("pid", type=int)
    parser.add_argument("identity")
    args = parser.parse_args()
    if args.pid <= 0:
        parser.error("a positive child PID is required")
    if args.action == "birth":
        print(birth(args.pid, int(args.identity)))
    else:
        stop(args.pid, args.identity)
