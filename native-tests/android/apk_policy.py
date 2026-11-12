"""Bind an APK manifest reference to its actual packaged extraction-rules file."""
import re


def extraction_resource(reference, table, package="com.takosumi.fixture.android"):
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
                file = re.fullmatch(r"\(file\) (res/\S+) type=XML", value[1])
                current["files"].append(file[1] if file else None)
    expected = "xml/data_extraction_rules"
    candidates = [r for r in resources if r["package"] == package
                  and r["name"] in (expected, f"{package}:{expected}")]
    if len(candidates) != 1:
        raise ValueError("Missing or ambiguous packaged extraction-rules resource")
    resource = candidates[0]
    same_id = [r for r in resources if r["id"] == resource["id"]]
    if len(same_id) != 1 or same_id[0] is not resource:
        raise ValueError("Ambiguous packaged extraction-rules resource ID")
    numeric = re.fullmatch(r"@ref/(0x[0-9a-fA-F]+)", reference or "")
    if not (reference == "@xml/data_extraction_rules"
            or numeric and int(numeric[1], 16) == resource["id"]):
        raise ValueError("Manifest does not reference the packaged extraction-rules resource")
    if resource["files"] != ["res/xml/data_extraction_rules.xml"]:
        raise ValueError("Unexpected or ambiguous extraction-rules file mapping")
    return resource["files"][0]
