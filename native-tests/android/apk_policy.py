"""Bind an APK manifest reference to its actual packaged extraction-rules file."""
import re


def resource_file(reference, table, name, path, package="com.takosumi.fixture.android", kind="XML"):
    current_package, current = None, None
    resources = []
    for line in table.splitlines():
        owner = re.fullmatch(r"\s*Package name=(\S+) id=(?:0x)?[0-9a-fA-F]+\s*", line)
        if owner:
            current_package, current = owner[1], None
            continue
        entry = re.fullmatch(r"\s*resource (0x[0-9a-fA-F]+) (\S+)(?:\s+.*)?", line)
        if entry:
            current = {"package": current_package, "id": int(entry[1], 16),
                       "name": entry[2], "files": []}
            resources.append(current)
        elif current is not None:
            value = re.fullmatch(r"\s*\([^)]*\) (.+)", line)
            if value:
                # aapt2 omits the suffix for raw/unknown FileReferences.
                # XML callers still require the explicit XML type.
                file = re.fullmatch(r"\(file\) (res/\S+?)(?: type=([A-Z_]+))?", value[1])
                current["files"].append(file[1] if file and (kind is None or file[2] == kind) else None)
    expected = name
    candidates = [r for r in resources if r["package"] == package
                  and r["name"] in (expected, f"{package}:{expected}")]
    if len(candidates) != 1:
        raise ValueError("Missing or ambiguous packaged resource: " + name)
    resource = candidates[0]
    same_id = [r for r in resources if r["id"] == resource["id"]]
    if len(same_id) != 1 or same_id[0] is not resource:
        raise ValueError("Ambiguous packaged resource ID: " + name)
    numeric = re.fullmatch(r"@ref/(0x[0-9a-fA-F]+)", reference or "")
    if not (reference == "@" + name
            or numeric and int(numeric[1], 16) == resource["id"]):
        raise ValueError("Wrong packaged resource reference: " + name)
    if resource["files"] != [path]:
        raise ValueError("Unexpected or ambiguous file mapping: " + name)
    return resource["files"][0]


def extraction_resource(reference, table, package="com.takosumi.fixture.android"):
    return resource_file(reference, table, "xml/data_extraction_rules",
                         "res/xml/data_extraction_rules.xml", package)
