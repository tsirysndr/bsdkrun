"""Unit tests for URL normalization and its WS derivation."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.transport import normalize_url, ws_url  # noqa: E402


class TestNormalizeUrl(unittest.TestCase):
    def test_adds_scheme_and_suffix(self):
        self.assertEqual(normalize_url("localhost:50052"), "http://localhost:50052/graphql")

    def test_strips_trailing_slashes(self):
        self.assertEqual(normalize_url("http://host:50052/"), "http://host:50052/graphql")
        self.assertEqual(normalize_url("http://host:50052///"), "http://host:50052/graphql")

    def test_leaves_existing_graphql_suffix(self):
        self.assertEqual(
            normalize_url("http://host:50052/graphql"), "http://host:50052/graphql"
        )
        self.assertEqual(
            normalize_url("http://host:50052/graphql/"), "http://host:50052/graphql"
        )

    def test_preserves_https(self):
        self.assertEqual(normalize_url("https://host:50052"), "https://host:50052/graphql")

    def test_trims_whitespace(self):
        self.assertEqual(normalize_url("  localhost:50052  "), "http://localhost:50052/graphql")

    def test_empty_input(self):
        self.assertEqual(normalize_url(""), "")
        self.assertEqual(normalize_url("   "), "")

    def test_case_insensitive_scheme_and_suffix(self):
        self.assertEqual(normalize_url("HTTPS://host/GraphQL"), "HTTPS://host/GraphQL")


class TestWsUrl(unittest.TestCase):
    def test_http_to_ws(self):
        self.assertEqual(ws_url("http://host:50052/graphql"), "ws://host:50052/graphql/ws")

    def test_https_to_wss(self):
        self.assertEqual(ws_url("https://host:50052/graphql"), "wss://host:50052/graphql/ws")

    def test_strips_trailing_slash_before_appending(self):
        self.assertEqual(ws_url("http://host:50052/graphql/"), "ws://host:50052/graphql/ws")


if __name__ == "__main__":
    unittest.main()
