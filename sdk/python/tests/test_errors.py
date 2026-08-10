"""Unit tests for the GraphQL-related error types."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.errors import AuthError, BsdkrunError, GraphQLError  # noqa: E402


class TestGraphQLError(unittest.TestCase):
    def test_is_a_bsdkrun_error(self):
        self.assertIsInstance(GraphQLError("boom"), BsdkrunError)

    def test_carries_an_optional_code(self):
        err = GraphQLError("boom", "INVALID_ARGUMENT")
        self.assertEqual(err.code, "INVALID_ARGUMENT")
        self.assertEqual(str(err), "boom")

    def test_code_defaults_to_none(self):
        self.assertIsNone(GraphQLError("boom").code)


class TestAuthError(unittest.TestCase):
    def test_is_a_graphql_error_with_unauthenticated_code(self):
        err = AuthError()
        self.assertIsInstance(err, GraphQLError)
        self.assertEqual(err.code, "UNAUTHENTICATED")

    def test_custom_message(self):
        err = AuthError("nope")
        self.assertEqual(str(err), "nope")
        self.assertEqual(err.code, "UNAUTHENTICATED")


if __name__ == "__main__":
    unittest.main()
