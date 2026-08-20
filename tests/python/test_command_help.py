import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "check_command_help", ROOT / "scripts" / "check_command_help.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class CommandHelpTests(unittest.TestCase):
    def test_documented_cli_matrix_is_unique_and_complete(self) -> None:
        self.assertEqual(len(MODULE.CLI_COMMANDS), 78)
        self.assertEqual(len(set(MODULE.CLI_COMMANDS)), len(MODULE.CLI_COMMANDS))
        self.assertIn(("legacy", "freeze"), MODULE.CLI_COMMANDS)
        self.assertIn(("search", "rebuild"), MODULE.CLI_COMMANDS)
        self.assertIn(("config", "validate"), MODULE.CLI_COMMANDS)
        self.assertIn(("scripts", "test"), MODULE.CLI_COMMANDS)
        self.assertIn(("notifications", "templates", "list"), MODULE.CLI_COMMANDS)

    def test_sanitized_environment_contains_only_path(self) -> None:
        with self.subTest(environment=MODULE.sanitized_environment()):
            self.assertEqual(set(MODULE.sanitized_environment()), {"PATH"})

    def test_external_operator_helpers_are_part_of_the_help_audit(self) -> None:
        self.assertTrue(callable(MODULE.check_rehearsal_help))
        self.assertTrue(callable(MODULE.check_integration_probe_help))


if __name__ == "__main__":
    unittest.main()
