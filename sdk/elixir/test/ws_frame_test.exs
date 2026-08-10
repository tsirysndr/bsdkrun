defmodule Bsdkrun.WsFrameTest do
  use ExUnit.Case, async: true

  alias Bsdkrun.WsFrame

  describe "accept_key/1" do
    test "matches the RFC 6455 §1.3 worked example" do
      # The exact key/accept pair from the spec itself.
      assert WsFrame.accept_key("dGhlIHNhbXBsZSBub25jZQ==") == "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    end
  end

  describe "random_key/0" do
    test "is a base64 encoding of 16 random bytes, and varies" do
      key = WsFrame.random_key()
      assert {:ok, decoded} = Base.decode64(key)
      assert byte_size(decoded) == 16
      refute WsFrame.random_key() == WsFrame.random_key()
    end
  end

  describe "encode/2 + decode/1 round trip" do
    test "a short text payload" do
      frame = WsFrame.encode_text("hello")
      assert {:ok, %{fin: 1, opcode: 0x1, payload: "hello"}, <<>>} = WsFrame.decode(frame)
    end

    test "an empty payload" do
      frame = WsFrame.encode_text("")
      assert {:ok, %{fin: 1, opcode: 0x1, payload: ""}, <<>>} = WsFrame.decode(frame)
    end

    test "a payload requiring the 16-bit extended length (>= 126 bytes)" do
      payload = String.duplicate("x", 1000)
      frame = WsFrame.encode(0x1, payload)
      assert {:ok, %{fin: 1, opcode: 0x1, payload: ^payload}, <<>>} = WsFrame.decode(frame)
    end

    test "a payload requiring the 64-bit extended length (> 65535 bytes)" do
      payload = String.duplicate("y", 70_000)
      frame = WsFrame.encode(0x1, payload)
      assert {:ok, %{fin: 1, opcode: 0x1, payload: ^payload}, <<>>} = WsFrame.decode(frame)
    end

    test "every client frame is masked (byte 2's high bit is set)" do
      frame = WsFrame.encode_text("masked?")
      <<_first, mask_bit::1, _len::7, _rest::binary>> = frame
      assert mask_bit == 1
    end

    test "leaves trailing bytes (the start of the next frame) in `rest`" do
      first = WsFrame.encode_text("one")
      second = WsFrame.encode_text("two")
      buffer = first <> second

      assert {:ok, %{payload: "one"}, rest} = WsFrame.decode(buffer)
      assert {:ok, %{payload: "two"}, <<>>} = WsFrame.decode(rest)
    end
  end

  describe "decode/1 with an incomplete buffer" do
    test "empty buffer" do
      assert WsFrame.decode(<<>>) == :incomplete
    end

    test "header present but payload truncated" do
      frame = WsFrame.encode_text("this is a full payload")
      truncated = binary_part(frame, 0, byte_size(frame) - 3)
      assert WsFrame.decode(truncated) == :incomplete
    end

    test "only the first byte present" do
      <<first, _rest::binary>> = WsFrame.encode_text("x")
      assert WsFrame.decode(<<first>>) == :incomplete
    end
  end

  describe "mask/2" do
    test "applying it twice with the same key is the identity" do
      key = :crypto.strong_rand_bytes(4)
      data = "round trip me"
      assert WsFrame.mask(WsFrame.mask(data, key), key) == data
    end

    test "cycles the 4-byte key across a longer payload" do
      key = <<1, 2, 3, 4>>
      data = <<10, 20, 30, 40, 50>>
      masked = WsFrame.mask(data, key)
      assert masked == <<10 |> Bitwise.bxor(1), 20 |> Bitwise.bxor(2), 30 |> Bitwise.bxor(3), 40 |> Bitwise.bxor(4), 50 |> Bitwise.bxor(1)>>
    end
  end
end
