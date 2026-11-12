"""SDK-free runtime-proof controls, never actual Swift loader qualification."""
import hashlib
import os
from pathlib import Path
import plistlib
import tempfile
import unittest

from swift_runtime import inspect, rpaths, require_swift_search


def commands(path, paths=("/usr/lib/swift", "@executable_path/Frameworks")):
    result = str(path) + ":\nLoad command 0\n     cmd LC_UUID\n cmdsize 24\n"
    for i, value in enumerate(paths, 1):
        result += f"Load command {i}\n          cmd LC_RPATH\n      cmdsize 40\n         path {value} (offset 12)\n"
    return result


class Controls(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp"))))

    def app(self):
        app = self.root / "build/Takosumi.app"
        (app / "Frameworks").mkdir(parents=True)
        with (app / "Info.plist").open("wb") as out:
            plistlib.dump({"CFBundleIdentifier": "com.takosumi.fixture.simulator", "CFBundleSupportedPlatforms": ["iPhoneSimulator"],
                          "CFBundleExecutable": "Takosumi"}, out)
        (app / "Takosumi").write_bytes(b"synthetic SDK-free executable")
        (app / "Frameworks/libswift_Concurrency.dylib").write_bytes(b"synthetic SDK-free runtime")
        return app

    def command(self, argv):
        path = Path(argv[-1])
        if "-l" in argv: return commands(path)
        return str(path) + ":\n\t@rpath/libswiftCore.dylib (compatibility version 1.0.0, current version 6.0.0)\n"

    def test_actual_load_command_paths_must_put_the_os_runtime_before_frameworks(self):
        path = self.root / "App"
        values = rpaths(commands(path), path)
        self.assertEqual(values, ["/usr/lib/swift", "@executable_path/Frameworks"])
        require_swift_search(values)
        for value in ([], ["@executable_path/Frameworks"], ["/usr/lib/swift"],
                      ["@executable_path/Frameworks", "/usr/lib/swift"],
                      ["@loader_path/Frameworks", "/usr/lib/swift", "@executable_path/Frameworks"]):
            with self.subTest(value=value), self.assertRaises(RuntimeError): require_swift_search(value)

    def test_a_string_outside_lc_rpath_is_not_a_runtime_search_entry(self):
        path = self.root / "App"
        text = str(path) + ":\nLoad command 0\n     cmd LC_LOAD_DYLIB\n    name /usr/lib/swift (offset 12)\n"
        self.assertEqual(rpaths(text, path), [])
        for value in ("", "foreign:\n" + commands(path), commands(path).replace(str(path) + ":", str(path) + " (architecture x86_64):"),
                      commands(path).replace("path /usr/lib/swift", "name /usr/lib/swift"),
                      commands(path).replace("path /usr/lib/swift (offset 12)", "path /usr/lib/swift (offset 12)\n path /foreign (offset 12)")):
            with self.subTest(value=value), self.assertRaises(RuntimeError): rpaths(value, path)
        selected = commands(path).replace(str(path) + ":", str(path) + " (architecture x86_64):")
        self.assertEqual(rpaths(selected, path, "x86_64"), ["/usr/lib/swift", "@executable_path/Frameworks"])
        with self.assertRaises(RuntimeError): rpaths(selected, path, "arm64")

    def test_proof_covers_actual_app_and_original_embedded_concurrency_bytes(self):
        app = self.app()
        proof = inspect(app, app.parent, self.command)
        self.assertEqual([n["path"] for n in proof["images"]], ["Takosumi", "Frameworks/libswift_Concurrency.dylib"])
        for node in proof["images"]:
            self.assertEqual(node["sha256"], hashlib.sha256((app / node["path"]).read_bytes()).hexdigest())
            self.assertIn("LC_RPATH", node["loadCommands"])
            self.assertIn("@rpath/libswiftCore.dylib", node["dependencies"])

    def test_foreign_bundle_links_and_missing_runtime_cannot_qualify(self):
        app = self.app()
        other = self.root / "other"
        other.mkdir()
        with self.assertRaises(RuntimeError): inspect(app, other, self.command)
        library = app / "Frameworks/libswift_Concurrency.dylib"
        library.unlink()
        with self.assertRaises(RuntimeError): inspect(app, app.parent, self.command)
        library.symlink_to(app / "Takosumi")
        with self.assertRaises(RuntimeError): inspect(app, app.parent, self.command)

    def test_missing_or_foreign_dependency_output_is_not_a_proof(self):
        app = self.app()
        for value in ("", "foreign:\n\tlibswiftCore.dylib\n", str(app / "Takosumi") + ":\n"):
            def command(argv):
                return self.command(argv) if "-l" in argv else value
            with self.subTest(value=value), self.assertRaises(RuntimeError): inspect(app, app.parent, command)

    def test_image_replacement_during_native_inspection_is_rejected(self):
        app = self.app()
        def command(argv):
            if "-L" in argv: (app / "Takosumi").write_bytes(b"changed while inspected")
            return self.command(argv)
        with self.assertRaises(RuntimeError): inspect(app, app.parent, command)


if __name__ == "__main__":
    unittest.main()
