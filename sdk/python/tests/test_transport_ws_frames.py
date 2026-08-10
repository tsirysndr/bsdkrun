"""Unit tests for the hand-rolled RFC 6455 handshake and frame codec.

These exercise `build_frame`/`parse_frame`/`compute_accept` as pure
functions — no socket involved — so the wire-level logic can be checked in
isolation from `WSTransport`'s threading.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from bsdkrun.transport import (  # noqa: E402
    OP_BINARY,
    OP_CLOSE,
    OP_PING,
    OP_TEXT,
    build_frame,
    compute_accept,
    parse_frame,
)


class TestComputeAccept(unittest.TestCase):
    def test_rfc6455_example(self):
        # The worked example from RFC 6455 §1.3.
        self.assertEqual(
            compute_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
        )


class TestFrameRoundtrip(unittest.TestCase):
    def test_small_masked_text_frame(self):
        payload = b'{"type":"connection_init"}'
        wire = build_frame(payload, opcode=OP_TEXT, mask=True)
        frame, consumed = parse_frame(wire)
        self.assertIsNotNone(frame)
        self.assertEqual(consumed, len(wire))
        self.assertTrue(frame.fin)
        self.assertEqual(frame.opcode, OP_TEXT)
        self.assertEqual(frame.payload, payload)

    def test_masked_frame_is_actually_masked_on_the_wire(self):
        # A masked frame's on-the-wire bytes must not contain the raw
        # payload verbatim (barring the astronomically unlikely all-zero
        # mask key) — this is the "get frame masking right" requirement.
        payload = b"A" * 32
        wire = build_frame(payload, opcode=OP_TEXT, mask=True)
        self.assertNotIn(payload, wire)
        # The mask bit (0x80) must be set on the second byte.
        self.assertTrue(wire[1] & 0x80)

    def test_unmasked_frame_has_mask_bit_clear(self):
        payload = b"hello"
        wire = build_frame(payload, opcode=OP_TEXT, mask=False)
        self.assertFalse(wire[1] & 0x80)
        frame, consumed = parse_frame(wire)
        self.assertEqual(frame.payload, payload)
        self.assertEqual(consumed, len(wire))

    def test_empty_payload(self):
        wire = build_frame(b"", opcode=OP_TEXT)
        frame, consumed = parse_frame(wire)
        self.assertEqual(frame.payload, b"")
        self.assertEqual(consumed, len(wire))

    def test_extended_16bit_length(self):
        payload = b"x" * 1000  # >= 126, < 65536
        wire = build_frame(payload, opcode=OP_BINARY)
        frame, consumed = parse_frame(wire)
        self.assertEqual(frame.opcode, OP_BINARY)
        self.assertEqual(frame.payload, payload)
        self.assertEqual(consumed, len(wire))

    def test_extended_64bit_length(self):
        payload = b"y" * 70000  # >= 65536
        wire = build_frame(payload, opcode=OP_BINARY)
        frame, consumed = parse_frame(wire)
        self.assertEqual(frame.payload, payload)
        self.assertEqual(consumed, len(wire))

    def test_ping_and_close_opcodes_roundtrip(self):
        for opcode in (OP_PING, OP_CLOSE):
            wire = build_frame(b"", opcode=opcode)
            frame, _ = parse_frame(wire)
            self.assertEqual(frame.opcode, opcode)

    def test_fin_bit(self):
        wire_fin = build_frame(b"abc", fin=True)
        wire_cont = build_frame(b"abc", fin=False)
        self.assertTrue(parse_frame(wire_fin)[0].fin)
        self.assertFalse(parse_frame(wire_cont)[0].fin)


class TestIncompleteFrames(unittest.TestCase):
    def test_empty_buffer(self):
        self.assertEqual(parse_frame(b""), (None, 0))

    def test_header_only(self):
        self.assertEqual(parse_frame(b"\x81"), (None, 0))

    def test_truncated_payload(self):
        wire = build_frame(b"hello world", mask=False)
        # Chop off the last few bytes of the payload.
        truncated = wire[:-3]
        self.assertEqual(parse_frame(truncated), (None, 0))

    def test_truncated_extended_length(self):
        payload = b"x" * 1000
        wire = build_frame(payload, mask=False)
        # Cut in the middle of the 16-bit extended length field.
        self.assertEqual(parse_frame(wire[:2]), (None, 0))

    def test_two_frames_back_to_back(self):
        first = build_frame(b"one", mask=False)
        second = build_frame(b"two", mask=False)
        buf = first + second
        frame1, consumed1 = parse_frame(buf)
        self.assertEqual(frame1.payload, b"one")
        self.assertEqual(consumed1, len(first))
        frame2, consumed2 = parse_frame(buf[consumed1:])
        self.assertEqual(frame2.payload, b"two")
        self.assertEqual(consumed2, len(second))


if __name__ == "__main__":
    unittest.main()
