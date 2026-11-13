"""Scoped test-CA resources for a private debug App; never device/global trust."""
import hashlib
import json
import copy
from pathlib import Path
import re
import stat
import xml.etree.ElementTree as ET

from apk_policy import resource_file

PACKAGE = "com.takosumi.fixture.android"
ANDROID = "{http://schemas.android.com/apk/res/android}"
CONFIG = """<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
  <domain-config cleartextTrafficPermitted="false">
    <domain includeSubdomains="false">127.0.0.1</domain>
    <domain includeSubdomains="false">localhost</domain>
    <trust-anchors><certificates src="@raw/sumi_fixture_ca" /></trust-anchors>
  </domain-config>
</network-security-config>
"""


def require(value, message):
    if not value:
        raise RuntimeError(message)


def certificate_bytes(path):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size < 32768,
            "Plain bounded public fixture certificate required")
    value = path.read_bytes()
    require(value.startswith(b"-----BEGIN CERTIFICATE-----\n")
            and value.endswith(b"-----END CERTIFICATE-----\n")
            and value.count(b"-----BEGIN CERTIFICATE-----") == 1 and b"PRIVATE KEY" not in value,
            "Only one public certificate may enter the debug fixture")
    return value


def policy(node):
    return [node.tag, sorted(node.attrib.items()), (node.text or "").strip(), [policy(c) for c in node]]


def domain_text_from_xmltree(decoded, text):
    """Bind actual aapt2 CDATA to the same leaves; never supply wanted hosts."""
    require(len(text) <= 16384, "Oversized network XML tree")
    result = copy.deepcopy(decoded)
    elements = []
    def walk(node, path=()):
        path = (*path, node.tag)
        elements.append((path, node))
        for child in node:
            walk(child, path)
    walk(result)
    current, position, attributes, texts = None, 0, {}, {}
    for line in text.splitlines():
        if not line.strip():
            continue
        element = re.fullmatch(r"( *)E: ([A-Za-z][A-Za-z0-9_-]*) \(line=[0-9]+\)", line)
        if element:
            require(position < len(elements), "Extra element in packaged network XML")
            path, node = elements[position]
            require(len(element[1]) == 4 * (len(path) - 1) and element[2] == node.tag,
                    "Packaged network XML differs from the decoded structure")
            current, position = position, position + 1
            attributes[current] = set()
            continue
        require(current is not None, "Network XML content precedes its root")
        path, node = elements[current]
        attribute = re.fullmatch(r"( *)A: ([A-Za-z][A-Za-z0-9_-]*)=(.+)", line)
        if attribute:
            require(len(attribute[1]) == 4 * (len(path) - 1) + 2
                    and attribute[2] in node.attrib and attribute[2] not in attributes[current],
                    "Wrong or duplicate packaged network XML attribute")
            attributes[current].add(attribute[2])
            continue
        content = re.fullmatch(r"( *)T: '([^']*)'", line)
        require(content is not None and path == ("network-security-config", "domain-config", "domain")
                and len(content[1]) == 4 * len(path) and current not in texts,
                "Unbound or repeated packaged domain text")
        value = content[2].strip()
        require(re.fullmatch(r"[A-Za-z0-9.:-]+", value), "Missing or malformed actual domain text")
        previous = (node.text or "").strip()
        require(not previous or previous == value, "Independent network decoders disagree on domain text")
        node.text = value
        texts[current] = value
    require(position == len(elements)
            and all(attributes.get(i) == set(node.attrib) for i, (_, node) in enumerate(elements)),
            "Incomplete packaged network XML structure/attributes")
    domains = [i for i, (path, _) in enumerate(elements)
               if path == ("network-security-config", "domain-config", "domain")]
    require(len(domains) == 2 and set(texts) == set(domains), "Exactly two actual domain leaves are required")
    return result


def verify_network(app, table, xml, archive, certificate, xmltree):
    path = resource_file(app.get(ANDROID + "networkSecurityConfig"), table,
                         "xml/sumi_fixture_network_security", "res/xml/sumi_fixture_network_security.xml")
    root = xml(path)
    actual_tree = xmltree(path)
    try:
        root = domain_text_from_xmltree(root, actual_tree)
    except RuntimeError as error:
        raise RuntimeError(str(error) + "; aapt2=" + json.dumps(actual_tree[:16384])) from error
    expected = ET.fromstring(CONFIG)
    anchors = root.findall("domain-config/trust-anchors/certificates")
    require(len(anchors) == 1, "The fixture must have exactly one scoped trust anchor")
    raw = resource_file(anchors[0].get("src"), table, "raw/sumi_fixture_ca",
                        "res/raw/sumi_fixture_ca.pem", kind=None)
    require(archive.read(raw) == certificate_bytes(certificate), "Packaged test CA differs from this owned run")
    anchors[0].set("src", "@raw/sumi_fixture_ca")
    actual, wanted = policy(root), policy(expected)
    require(actual == wanted, "Fixture trust must be limited to the two exact HTTPS hosts: "
            + json.dumps({"actual": actual, "expected": wanted}, separators=(",", ":")))
    return {"domains": ["127.0.0.1", "localhost"], "domainTextSource": "same packaged XML via aapt2 xmltree",
            "certificateSHA256": hashlib.sha256(archive.read(raw)).hexdigest()}


class Overlay:
    def __init__(self, root, certificate):
        generated = root / "src-tauri/gen/android"
        require(generated.is_dir() and not generated.is_symlink(), "Owned generated Android project required")
        self.main = generated / "app/src/main"
        require(self.main.resolve(strict=True).is_relative_to(generated.resolve(strict=True)), "Foreign App source")
        for path in (generated / "app", generated / "app/src", self.main):
            require(path.is_dir() and not path.is_symlink(), "Plain generated App directories required")
        gradle = (generated / "app/build.gradle.kts").read_text()
        require(re.findall(r'applicationId\s*=\s*"([^"]+)"', gradle) == [PACKAGE], "Not the private fixture project")
        self.manifest = self.main / "AndroidManifest.xml"
        info = self.manifest.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, "Plain private manifest required")
        self.before = self.manifest.read_bytes()
        self.manifest_identity = (info.st_dev, info.st_ino)
        manifest = ET.fromstring(self.before)
        app = manifest.find("application")
        require(app is not None and app.get(ANDROID + "networkSecurityConfig") is None,
                "Refusing to replace an existing network security policy")
        self.files = {self.main / "res/xml/sumi_fixture_network_security.xml": CONFIG.encode(),
                      self.main / "res/raw/sumi_fixture_ca.pem": certificate_bytes(certificate)}
        for path in self.files:
            require(not path.exists() and not path.is_symlink(), "Existing fixture trust resource refused")
            require(path.parent.resolve().is_relative_to(generated.resolve()), "Foreign resource directory")
            parent = path.parent
            while parent != self.main:
                require(not parent.is_symlink(), "Linked resource directory refused")
                parent = parent.parent
        ET.register_namespace("android", ANDROID[1:-1])
        app.set(ANDROID + "networkSecurityConfig", "@xml/sumi_fixture_network_security")
        self.after = ET.tostring(manifest, encoding="utf-8", xml_declaration=True)
        self.identities = None

    def custody(self):
        paths = {self.manifest, *self.files}
        for path in list(paths):
            parent = path.parent
            while parent != self.main.parent:
                paths.add(parent)
                parent = parent.parent
        values = {}
        for path in paths:
            info = path.lstat()
            require(not stat.S_ISLNK(info.st_mode), "Linked fixture source refused")
            values[path] = (info.st_dev, info.st_ino)
        require(values[self.manifest] == self.manifest_identity, "Fixture manifest was replaced")
        return values

    def apply(self):
        require(self.manifest.read_bytes() == self.before, "Manifest changed before fixture trust setup")
        for path in self.files:
            require(not path.exists() and not path.is_symlink(), "Fixture resource became occupied")
        for path, value in self.files.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as stream:
                stream.write(value)
        self.identities = self.custody()
        self.manifest.write_bytes(self.after)

    def restore(self):
        require(self.identities is not None and self.custody() == self.identities,
                "Fixture source custody changed; preserving private resources")
        require(self.manifest.read_bytes() == self.after, "Refusing to overwrite a changed fixture manifest")
        for path, value in self.files.items():
            require(path.is_file() and not path.is_symlink() and path.read_bytes() == value,
                    "Refusing to remove a changed fixture resource")
        self.manifest.write_bytes(self.before)
        for path in self.files:
            path.unlink()
