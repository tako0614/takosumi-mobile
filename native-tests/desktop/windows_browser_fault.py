"""An explicit fault on one fixture-owned Windows browser, using held handles.

No process-name lookup or kill-by-PID fallback. The App, guard and browser
remain bound to their kernel objects while ancestry, images and creation
times are checked. Only the browser handle can carry PROCESS_TERMINATE.
"""
import ctypes
import ntpath
from pathlib import Path
import stat
import uuid

from windows_smoke import Failure, ProcessTree, require

FAULT_EXIT = 73
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 258


def recovery_paths(process, app_data, runtime):
    """Bind persistent profile data and ephemeral guard state separately."""
    def resolved(path):
        try:
            return Path(path).resolve(strict=True)
        except OSError as error:
            raise Failure("A recovery path is unavailable") from error

    profile = process["profile"]
    require(isinstance(profile, str) and str(uuid.UUID(profile)) == profile, "Canonical recovery profile required")
    base = resolved(app_data)
    directory = base / "browser-process-experiment" / profile
    root, cache = directory / "cef-profiles", directory / "cef-profiles" / profile
    require(resolved(process["directory"]) == resolved(directory)
            and resolved(process["opened"]["nativeCache"]) == resolved(cache),
            "Recovery data/cache must match this case's exact App-data profile")
    receipt = Path(process["guardReceipt"])
    runtime = resolved(runtime)
    require(resolved(receipt).is_relative_to(runtime), "Guard receipt must belong to this case runtime")
    for path, is_directory in ((directory.parent, True), (directory, True), (root, True), (cache, True),
                               (receipt, False)):
        info = path.lstat()
        require(not (getattr(info, "st_file_attributes", 0) & 0x400) and not path.is_symlink()
                and (stat.S_ISDIR(info.st_mode) if is_directory else stat.S_ISREG(info.st_mode)),
                "Recovery requires plain case directories and receipt")
    return {"root": root, "cache": cache, "receipt": receipt}


def image_key(value):
    require(isinstance(value, str) and ntpath.isabs(value), "An absolute process image is required")
    if value.startswith("\\\\?\\UNC\\"):
        value = "\\\\" + value[8:]
    elif value.startswith("\\\\?\\"):
        value = value[4:]
    return ntpath.normcase(ntpath.normpath(value))


class WindowsProcesses:
    """The Win32 calls used by OwnedBrowser; constructed only on Windows.

    TerminateProcess is asynchronous: wait on the same handle afterwards.
    https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
    """
    def __init__(self, kernel32):
        self.kernel = kernel32
        handle, dword = ctypes.c_void_p, ctypes.c_uint32
        pointer = ctypes.POINTER(ctypes.c_uint64)
        for name, result, arguments in (
            ("OpenProcess", handle, [dword, ctypes.c_int, dword]),
            ("GetProcessId", dword, [handle]),
            ("GetProcessTimes", ctypes.c_int, [handle, pointer, pointer, pointer, pointer]),
            ("QueryFullProcessImageNameW", ctypes.c_int,
             [handle, dword, ctypes.c_wchar_p, ctypes.POINTER(dword)]),
            ("WaitForSingleObject", dword, [handle, dword]),
            ("GetExitCodeProcess", ctypes.c_int, [handle, ctypes.POINTER(dword)]),
            ("TerminateProcess", ctypes.c_int, [handle, ctypes.c_uint]),
            ("CloseHandle", ctypes.c_int, [handle]),
        ):
            function = getattr(kernel32, name)
            function.restype, function.argtypes = result, arguments

    def open(self, pid, terminate=False):
        # SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION; the one fault
        # target additionally receives PROCESS_TERMINATE, never an owner.
        value = self.kernel.OpenProcess(0x100000 | 0x1000 | int(terminate), False, pid)
        require(bool(value), "Cannot hold the fixture process")
        return value

    def identity(self, handle):
        pid = self.kernel.GetProcessId(handle)
        times = [ctypes.c_uint64() for _ in range(4)]
        require(pid > 0 and self.kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)),
                "Cannot read the held process creation time")
        text, size = ctypes.create_unicode_buffer(32768), ctypes.c_uint32(32768)
        require(self.kernel.QueryFullProcessImageNameW(handle, 0, text, ctypes.byref(size)),
                "Cannot read the held process image")
        return {"pid": pid, "created": times[0].value, "image": image_key(text.value)}

    def live(self, handle):
        result = self.kernel.WaitForSingleObject(handle, 0)
        require(result in (WAIT_OBJECT_0, WAIT_TIMEOUT), "Cannot establish held process liveness")
        return result == WAIT_TIMEOUT

    def parents(self):
        return dict(ProcessTree(self.kernel, 0).pairs())

    def terminate(self, handle):
        require(self.kernel.TerminateProcess(handle, FAULT_EXIT), "Owned browser termination failed")

    def wait(self, handle, milliseconds):
        require(self.kernel.WaitForSingleObject(handle, milliseconds) == WAIT_OBJECT_0,
                "Held process did not finish within the fixture bound")

    def exit_code(self, handle):
        code = ctypes.c_uint32()
        require(self.kernel.GetExitCodeProcess(handle, ctypes.byref(code)), "Cannot read held process exit")
        return code.value

    def close(self, handle):
        self.kernel.CloseHandle(handle)


class OwnedBrowser:
    """Hold one App -> guard -> browser lineage through an explicit fault."""
    def __init__(self, api, pids, images, allow_fault=False):
        self.api, self.pids, self.allow_fault = api, dict(pids), allow_fault
        self.images = {role: image_key(images[role]) for role in ("app", "guard", "browser")}
        require(set(self.pids) == set(self.images)
                and all(type(pid) is int and pid > 0 for pid in self.pids.values())
                and len(set(self.pids.values())) == 3, "Distinct fixture process identities required")
        self.handles, self.identities = {}, None
        try:
            for role, pid in self.pids.items():
                self.handles[role] = api.open(pid, terminate=allow_fault and role == "browser")
            self.validate()
        except BaseException:
            self.close()
            raise

    def validate(self):
        require(len(self.handles) == 3, "The fixture's process handles are closed")
        identities = {role: self.api.identity(handle) for role, handle in self.handles.items()}
        require(all(self.api.live(handle) for handle in self.handles.values()), "A held owner or browser ended")
        for role, identity in identities.items():
            require(identity["pid"] == self.pids[role] and identity["image"] == self.images[role]
                    and type(identity["created"]) is int and identity["created"] > 0,
                    "A held process does not match its fixture binding")
        parents = self.api.parents()
        require(parents.get(self.pids["browser"]) == self.pids["guard"]
                and parents.get(self.pids["guard"]) == self.pids["app"], "Fixture ancestry changed")
        require(identities["app"]["created"] <= identities["guard"]["created"]
                <= identities["browser"]["created"], "A child predates its held owner")
        require(self.identities is None or self.identities == identities, "Held process identity changed")
        require(all(self.api.live(handle) for handle in self.handles.values()),
                "A held owner ended during ancestry inspection")
        self.identities = identities
        return identities

    def fault(self):
        require(self.allow_fault, "This lineage is observation-only")
        self.validate()
        self.api.terminate(self.handles["browser"])
        self.api.wait(self.handles["browser"], 10000)
        require(self.api.exit_code(self.handles["browser"]) == FAULT_EXIT, "Unexpected browser fault exit")
        return {"operation": "TerminateProcess", "exit": FAULT_EXIT, "identities": self.identities}

    def guard_ended(self):
        self.api.wait(self.handles["guard"], 10000)
        require(self.api.exit_code(self.handles["guard"]) == 1, "The crash guard must report failure")
        require(self.api.live(self.handles["app"]), "The owning App ended during crash cleanup")

    def close(self):
        for handle in self.handles.values():
            self.api.close(handle)
        self.handles.clear()

    def __enter__(self):
        return self

    def __exit__(self, *error):
        self.close()
