package bsdkrun

import (
	"bytes"
	"testing"
)

// -- URL normalization ------------------------------------------------------

func TestNormalizeURL(t *testing.T) {
	cases := map[string]string{
		"localhost:50052":           "http://localhost:50052/graphql",
		"http://host:50052/":        "http://host:50052/graphql",
		"http://host:50052///":      "http://host:50052/graphql",
		"https://host":              "https://host/graphql",
		"http://host:50052/graphql": "http://host:50052/graphql",
		"  http://host:50052  ":     "http://host:50052/graphql",
		"HTTPS://host/GraphQL":      "HTTPS://host/GraphQL",
		"":                          "",
	}
	for in, want := range cases {
		if got := normalizeURL(in); got != want {
			t.Errorf("normalizeURL(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestWSEndpoint(t *testing.T) {
	cases := map[string]string{
		"http://host:50052/graphql":  "ws://host:50052/graphql/ws",
		"https://host/graphql":       "wss://host/graphql/ws",
		"http://host:50052/graphql/": "ws://host:50052/graphql/ws",
	}
	for in, want := range cases {
		if got := wsEndpoint(in); got != want {
			t.Errorf("wsEndpoint(%q) = %q, want %q", in, got, want)
		}
	}
}

// -- handshake --------------------------------------------------------------

func TestComputeAcceptRFCVector(t *testing.T) {
	// The worked example from RFC 6455 §1.3.
	if got := computeAccept("dGhlIHNhbXBsZSBub25jZQ=="); got != "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=" {
		t.Fatalf("computeAccept: %q", got)
	}
}

// -- frame codec ------------------------------------------------------------

func TestFrameRoundtripMasked(t *testing.T) {
	payload := []byte(`{"type":"connection_init"}`)
	raw := buildFrame(payload, opText, true, true)
	frame, consumed := parseFrame(raw)
	if frame == nil || consumed != len(raw) {
		t.Fatalf("frame=%v consumed=%d len=%d", frame, consumed, len(raw))
	}
	if !frame.fin || frame.opcode != opText || !bytes.Equal(frame.payload, payload) {
		t.Fatalf("%+v", frame)
	}
}

func TestFrameRoundtripUnmaskedServerFrame(t *testing.T) {
	payload := []byte("hello")
	raw := buildFrame(payload, opText, false, true)
	// The unmasked wire shape: FIN|text, then a bare length.
	if raw[0] != 0x81 || raw[1] != byte(len(payload)) {
		t.Fatalf("header: %x", raw[:2])
	}
	frame, consumed := parseFrame(raw)
	if frame == nil || consumed != len(raw) || !bytes.Equal(frame.payload, payload) {
		t.Fatalf("frame=%+v consumed=%d", frame, consumed)
	}
}

func TestFrameExtendedLength16(t *testing.T) {
	payload := bytes.Repeat([]byte("x"), 200)
	raw := buildFrame(payload, opBinary, true, true)
	if raw[1]&0x7F != 126 {
		t.Fatalf("expected the 16-bit length marker, header %x", raw[:2])
	}
	frame, consumed := parseFrame(raw)
	if frame == nil || consumed != len(raw) || len(frame.payload) != 200 {
		t.Fatalf("frame=%v consumed=%d", frame, consumed)
	}
	if !bytes.Equal(frame.payload, payload) {
		t.Fatal("payload mismatch")
	}
}

func TestFrameExtendedLength64(t *testing.T) {
	payload := bytes.Repeat([]byte("y"), 70000)
	raw := buildFrame(payload, opBinary, false, true)
	if raw[1]&0x7F != 127 {
		t.Fatalf("expected the 64-bit length marker, header %x", raw[:2])
	}
	frame, consumed := parseFrame(raw)
	if frame == nil || consumed != len(raw) || !bytes.Equal(frame.payload, payload) {
		t.Fatalf("frame=%v consumed=%d", frame, consumed)
	}
}

func TestFramePartialReturnsNothing(t *testing.T) {
	raw := buildFrame([]byte("hello world"), opText, true, true)
	for cut := range len(raw) {
		frame, consumed := parseFrame(raw[:cut])
		if frame != nil || consumed != 0 {
			t.Fatalf("cut=%d yielded frame=%v consumed=%d", cut, frame, consumed)
		}
	}
}

func TestFrameControlOpcodes(t *testing.T) {
	raw := buildFrame([]byte("ping-payload"), opPing, true, true)
	frame, _ := parseFrame(raw)
	if frame == nil || frame.opcode != opPing || string(frame.payload) != "ping-payload" {
		t.Fatalf("%+v", frame)
	}
	closeFrame, _ := parseFrame(buildFrame(nil, opClose, true, true))
	if closeFrame == nil || closeFrame.opcode != opClose {
		t.Fatalf("%+v", closeFrame)
	}
}

func TestFrameStreamConsumesSequentially(t *testing.T) {
	one := buildFrame([]byte("one"), opText, false, true)
	two := buildFrame([]byte("two"), opText, false, true)
	buf := append(append([]byte{}, one...), two...)

	frame, consumed := parseFrame(buf)
	if frame == nil || string(frame.payload) != "one" {
		t.Fatalf("first: %+v", frame)
	}
	frame, consumed2 := parseFrame(buf[consumed:])
	if frame == nil || string(frame.payload) != "two" || consumed+consumed2 != len(buf) {
		t.Fatalf("second: %+v", frame)
	}
}
