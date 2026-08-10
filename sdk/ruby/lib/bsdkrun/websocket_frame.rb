# frozen_string_literal: true

module Bsdkrun
  # RFC 6455 §5.2 frame encode/decode, hand-rolled because the Ruby standard
  # library has no WebSocket client.
  #
  # Deliberately narrow: only what {WsClient} needs to speak
  # +graphql-transport-ws+, which exchanges small JSON text frames. Fragmented
  # messages (a logical message split across CONTINUATION frames) are
  # unsupported — {read} returns whatever single frame arrives, and a caller
  # that gets a non-FIN frame is expected to treat it as an error. In
  # practice every graphql-ws message this SDK deals with is small enough
  # that no server has ever been observed to fragment one.
  module WebSocketFrame
    OPCODES = {
      continuation: 0x0,
      text: 0x1,
      binary: 0x2,
      close: 0x8,
      ping: 0x9,
      pong: 0xA
    }.freeze

    module_function

    # Build a complete frame.
    #
    # @param opcode [Symbol] a key of {OPCODES}.
    # @param payload [String] the frame body (binary-safe).
    # @param mask [Boolean] +true+ for client->server frames (RFC 6455
    #   requires it — the mask key is randomly generated per frame); +false+
    #   for server->client frames, which MUST NOT be masked.
    # @return [String] the raw bytes to write to the socket.
    def build(opcode:, payload: "".b, mask: true)
      payload = payload.to_s.dup.force_encoding(Encoding::BINARY)
      code = OPCODES.fetch(opcode) { raise ArgumentError, "unknown opcode #{opcode.inspect}" }

      frame = +"".b
      frame << [0x80 | code].pack("C") # FIN=1, RSV1-3=0
      frame << length_bytes(payload.bytesize, mask)

      if mask
        key = Random.bytes(4)
        frame << key
        frame << apply_mask(payload, key)
      else
        frame << payload
      end
      frame
    end

    # Read exactly one frame from +io+ (anything responding to +#read(n)+ —
    # a +TCPSocket+, an +OpenSSL::SSL::SSLSocket+, or a +StringIO+ in tests).
    #
    # @param io [#read]
    # @return [Array(Boolean, Integer, String)] +[fin, opcode, payload]+ —
    #   +payload+ is already unmasked.
    # @raise [EOFError] if the socket closes mid-frame.
    def read(io)
      b0, b1 = read_exact(io, 2).unpack("C2")
      fin = (b0 & 0x80) != 0
      opcode = b0 & 0x0F
      masked = (b1 & 0x80) != 0
      len = b1 & 0x7F

      len = read_exact(io, 2).unpack1("n") if len == 126
      len = read_exact(io, 8).unpack1("Q>") if len == 127

      mask_key = masked ? read_exact(io, 4) : nil
      payload = len.positive? ? read_exact(io, len) : "".b
      payload = apply_mask(payload, mask_key) if masked

      [fin, opcode, payload]
    end

    # XOR-mask (or, symmetrically, unmask) +payload+ against a 4-byte +key+.
    # @param payload [String]
    # @param key [String] exactly 4 bytes.
    # @return [String]
    def apply_mask(payload, key)
      out = payload.b
      key_bytes = key.bytes
      bytes = out.bytes
      bytes.each_index { |i| bytes[i] ^= key_bytes[i % 4] }
      bytes.pack("C*")
    end

    # @!visibility private
    def length_bytes(len, mask)
      mask_bit = mask ? 0x80 : 0x00
      if len < 126
        [mask_bit | len].pack("C")
      elsif len < 65_536
        [mask_bit | 126].pack("C") + [len].pack("n")
      else
        [mask_bit | 127].pack("C") + [len].pack("Q>")
      end
    end

    # @!visibility private
    #
    # +io.read(n)+ already blocks until +n+ bytes are available or EOF, but a
    # partial read (fewer than +n+ bytes, non-nil) is technically possible
    # per the IO contract, so this loops to be sure.
    def read_exact(io, n)
      buf = "".b
      while buf.bytesize < n
        chunk = io.read(n - buf.bytesize)
        raise EOFError, "socket closed mid-frame" if chunk.nil?

        buf << chunk
      end
      buf
    end
  end
end
