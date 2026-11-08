"""Hold the expected executable before launch; accept only aliases of that file.

Chromium hard-links its main executable into a code-sign clone. Pathname
equality rejects that legitimate image. This is file identity, not bundle
signature verification or atomic proof of the process's running vnode.
"""
import os
import stat


class PinnedImage:
    def __init__(self, path):
        self.file = path.open("rb")
        try:
            self.identity = self.regular_identity(self.file)
        except Exception:
            self.file.close()
            raise

    @staticmethod
    def regular_identity(file):
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("Expected a regular executable file")
        return info.st_dev, info.st_ino

    def matches(self, path):
        try:
            with path.open("rb") as actual:
                return self.regular_identity(actual) == self.identity
        except (OSError, ValueError):
            return False

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.file.close()
