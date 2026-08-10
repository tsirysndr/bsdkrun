# frozen_string_literal: true

require "minitest/autorun"
require "stringio"
require "bsdkrun"

# Frame-level tests for the hand-rolled RFC 6455 codec: pure methods, no
# socket involved, so masking/length-encoding bugs are caught without the
# noise of a real connection.
class TestWebSocketFrame < Minitest::Test
  Frame = Bsdkrun::WebSocketFrame

  def test_build_sets_fin_and_opcode
    frame = Frame.build(opcode: :text, payload: "hi", mask: false)
    first_byte = frame.getbyte(0)
    assert_equal(0x80, first_byte & 0x80, "FIN bit must be set")
    assert_equal(0x1, first_byte & 0x0F, "opcode must be text (0x1)")
  end

  def test_build_unmasked_small_payload_round_trips
    frame = Frame.build(opcode: :text, payload: "hello", mask: false)
    fin, opcode, payload = Frame.read(StringIO.new(frame))
    assert fin
    assert_equal(Frame::OPCODES[:text], opcode)
    assert_equal("hello", payload)
  end

  def test_build_masked_frame_is_masked_on_the_wire_but_reads_back_clear
    frame = Frame.build(opcode: :text, payload: "secret", mask: true)
    # mask bit is set on the length byte
    assert_equal(0x80, frame.getbyte(1) & 0x80)
    # the raw bytes on the wire must not contain the plaintext payload
    refute_includes(frame, "secret")

    _fin, _opcode, payload = Frame.read(StringIO.new(frame))
    assert_equal("secret", payload)
  end

  def test_length_encoding_126_boundary
    payload = "x" * 200 # >= 126, < 65536: 2-byte extended length
    frame = Frame.build(opcode: :binary, payload: payload, mask: false)
    assert_equal(126, frame.getbyte(1) & 0x7F)
    _fin, opcode, decoded = Frame.read(StringIO.new(frame))
    assert_equal(Frame::OPCODES[:binary], opcode)
    assert_equal(payload, decoded)
  end

  def test_length_encoding_127_boundary
    payload = "y" * 70_000 # >= 65536: 8-byte extended length
    frame = Frame.build(opcode: :binary, payload: payload, mask: false)
    assert_equal(127, frame.getbyte(1) & 0x7F)
    _fin, _opcode, decoded = Frame.read(StringIO.new(frame))
    assert_equal(payload, decoded)
  end

  def test_apply_mask_is_its_own_inverse
    key = "abcd"
    masked = Frame.apply_mask("payload-bytes", key)
    refute_equal("payload-bytes", masked)
    assert_equal("payload-bytes", Frame.apply_mask(masked, key))
  end

  def test_read_raises_eof_on_truncated_frame
    # A header claiming 10 bytes of payload, but none supplied.
    truncated = [0x81, 10].pack("C2")
    assert_raises(EOFError) { Frame.read(StringIO.new(truncated)) }
  end

  def test_ping_and_pong_opcodes_round_trip
    frame = Frame.build(opcode: :ping, payload: "", mask: false)
    _fin, opcode, = Frame.read(StringIO.new(frame))
    assert_equal(Frame::OPCODES[:ping], opcode)
  end

  def test_unknown_opcode_raises
    assert_raises(ArgumentError) { Frame.build(opcode: :bogus, payload: "x") }
  end
end
