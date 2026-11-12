"""SDK-free controls for binding a compiled manifest reference to APK rules."""
import unittest

from apk_policy import extraction_resource


TABLE = """Package name=com.takosumi.fixture.android id=7f
  type xml id=12 entryCount=2
    resource 0x7f120000 xml/data_extraction_rules
      () (file) res/xml/data_extraction_rules.xml type=XML
    resource 0x7f120001 xml/file_paths
      () (file) res/xml/file_paths.xml type=XML
"""


class CompiledReferenceTests(unittest.TestCase):
    def test_resource_visibility_flag_is_not_part_of_the_name(self):
        table = TABLE.replace("xml/data_extraction_rules\n", "xml/data_extraction_rules PUBLIC\n")
        self.assertEqual(extraction_resource("@ref/0x7f120000", table),
                         "res/xml/data_extraction_rules.xml")

    def test_resolves_compiled_id_to_the_exact_file(self):
        self.assertEqual(extraction_resource("@ref/0x7f120000", TABLE),
                         "res/xml/data_extraction_rules.xml")

    def test_named_reference_still_requires_the_actual_mapping(self):
        self.assertEqual(extraction_resource("@xml/data_extraction_rules", TABLE),
                         "res/xml/data_extraction_rules.xml")
        with self.assertRaises(ValueError):
            extraction_resource("@xml/data_extraction_rules", "")

    def test_another_existing_resource_id_is_not_the_policy(self):
        with self.assertRaises(ValueError):
            extraction_resource("@ref/0x7f120001", TABLE)

    def test_absent_and_malformed_references_fail(self):
        for value in (None, "", "@ref/0xgarbage", "@xml/file_paths"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                extraction_resource(value, TABLE)

    def test_rejects_wrong_package_and_duplicate_resource(self):
        for table in (TABLE.replace("com.takosumi.fixture.android", "com.other"), TABLE + TABLE):
            with self.subTest(table=table), self.assertRaises(ValueError):
                extraction_resource("@ref/0x7f120000", table)

    def test_a_second_name_cannot_alias_the_same_numeric_id(self):
        table = TABLE.replace("0x7f120001 xml/file_paths", "0x7f120000 xml/file_paths")
        for reference in ("@ref/0x7f120000", "@xml/data_extraction_rules"):
            with self.subTest(reference=reference), self.assertRaises(ValueError):
                extraction_resource(reference, table)

    def test_rejects_missing_foreign_and_ambiguous_file_mappings(self):
        original = "      () (file) res/xml/data_extraction_rules.xml type=XML\n"
        for changed in ("", original.replace("data_extraction_rules.xml", "file_paths.xml"),
                        original + original.replace("()", "(v31)")):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                extraction_resource("@ref/0x7f120000", TABLE.replace(original, changed))

    def test_alias_config_cannot_hide_beside_the_expected_file(self):
        original = "      () (file) res/xml/data_extraction_rules.xml type=XML\n"
        for changed in ("      () @0x7f120001\n", original + "      (v31) @0x7f120001\n"):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                extraction_resource("@ref/0x7f120000", TABLE.replace(original, changed))


if __name__ == "__main__":
    unittest.main()
