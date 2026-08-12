package bsdkrun

import (
	"bytes"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// The GraphQL transport for talking to a remote bsdkrund. Two halves:
//
//   - httpRequest — a plain POST for queries and mutations over net/http.
//     Mirrors the Python SDK's http_request (headers, error mapping,
//     transport-failure wrapping).
//
//   - wsTransport — subscriptions need a persistent socket speaking the
//     graphql-transport-ws protocol, and the standard library has no
//     WebSocket client at all. This hand-rolls RFC 6455: the HTTP Upgrade
//     handshake (computeAccept) and frame codec (buildFrame, parseFrame)
//     are pure/stateless so they can be unit tested without a socket;
//     wsTransport wraps them with a background reader goroutine.
//
// Only the standard library is used throughout, matching the rest of this
// dependency-free SDK.

// EnvURL and EnvToken are the environment variables ClientFromEnv reads.
const (
	EnvURL   = "BSDKRUN_URL"
	EnvToken = "BSDKRUN_TOKEN"
)

// ---------------------------------------------------------------------------
// URL handling
// ---------------------------------------------------------------------------

var (
	schemeRe        = regexp.MustCompile(`(?i)^https?://`)
	graphqlSuffixRe = regexp.MustCompile(`(?i)/graphql$`)
)

// normalizeURL turns what a person pastes into a full GraphQL endpoint URL:
// trim, assume http:// when no scheme is given (people type
// "localhost:50052"), strip trailing slashes, and append /graphql unless
// the path already ends with it.
func normalizeURL(raw string) string {
	s := strings.TrimSpace(raw)
	if s == "" {
		return s
	}
	if !schemeRe.MatchString(s) {
		s = "http://" + s
	}
	s = strings.TrimRight(s, "/")
	if !graphqlSuffixRe.MatchString(s) {
		s += "/graphql"
	}
	return s
}

// wsEndpoint derives the subscriptions URL from a normalized HTTP endpoint
// URL: http:// becomes ws://, https:// becomes wss://; trailing slashes on
// the path are stripped and /ws is appended — e.g.
// http://host:50052/graphql -> ws://host:50052/graphql/ws.
func wsEndpoint(httpURL string) string {
	scheme, rest := "ws://", httpURL
	switch {
	case strings.HasPrefix(httpURL, "https://"):
		scheme, rest = "wss://", strings.TrimPrefix(httpURL, "https://")
	case strings.HasPrefix(httpURL, "http://"):
		rest = strings.TrimPrefix(httpURL, "http://")
	}
	return scheme + strings.TrimRight(rest, "/") + "/ws"
}

// ---------------------------------------------------------------------------
// HTTP transport (queries + mutations)
// ---------------------------------------------------------------------------

// httpRequest runs a query or mutation over HTTP and returns data. It
// returns an *AuthError on an HTTP 401 or a GraphQL error whose
// extensions.code is "UNAUTHENTICATED", and a *GraphQLError for any other
// GraphQL error or for a transport-level failure (the daemon could not be
// reached at all).
func httpRequest(endpoint, token, query string, variables map[string]any) (map[string]any, error) {
	if variables == nil {
		variables = map[string]any{}
	}
	body, err := json.Marshal(map[string]any{"query": query, "variables": variables})
	if err != nil {
		return nil, &GraphQLError{Message: fmt.Sprintf("cannot encode the request — %v", err)}
	}

	request, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return nil, &GraphQLError{Message: fmt.Sprintf("invalid daemon URL %q — %v", endpoint, err)}
	}
	request.Header.Set("content-type", "application/json")
	request.Header.Set("authorization", "Bearer "+token)

	// A non-2xx response still carries a JSON body worth parsing (the
	// daemon returns structured GraphQL errors even on a 4xx/5xx), so only
	// a transport failure is fatal here.
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		return nil, &GraphQLError{
			Message: fmt.Sprintf("cannot reach the bsdkrun daemon at %s — %v", endpoint, err),
		}
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(response.Body)
	if err != nil {
		return nil, &GraphQLError{
			Message: fmt.Sprintf("cannot reach the bsdkrun daemon at %s — %v", endpoint, err),
		}
	}

	if response.StatusCode == http.StatusUnauthorized {
		return nil, &AuthError{}
	}

	var parsed struct {
		Data   map[string]any `json:"data"`
		Errors []struct {
			Message    string `json:"message"`
			Extensions struct {
				Code string `json:"code"`
			} `json:"extensions"`
		} `json:"errors"`
	}
	if err := json.Unmarshal(raw, &parsed); err != nil {
		return nil, &GraphQLError{
			Message: fmt.Sprintf("the daemon returned a non-JSON response (%d)", response.StatusCode),
		}
	}

	if len(parsed.Errors) > 0 {
		first := parsed.Errors[0]
		message := first.Message
		if message == "" {
			message = "unknown error"
		}
		if first.Extensions.Code == "UNAUTHENTICATED" {
			return nil, &AuthError{Message: message}
		}
		return nil, &GraphQLError{Message: message, Code: first.Extensions.Code}
	}

	if parsed.Data == nil {
		return map[string]any{}, nil
	}
	return parsed.Data, nil
}

// ---------------------------------------------------------------------------
// WebSocket handshake (RFC 6455 §1.3)
// ---------------------------------------------------------------------------

const (
	wsGUID        = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
	wsSubprotocol = "graphql-transport-ws"
)

// makeWSKey returns a fresh Sec-WebSocket-Key (16 random bytes, base64).
func makeWSKey() string {
	key := make([]byte, 16)
	_, _ = rand.Read(key)
	return base64.StdEncoding.EncodeToString(key)
}

// computeAccept returns the Sec-WebSocket-Accept value the server must
// answer key with.
func computeAccept(key string) string {
	digest := sha1.Sum([]byte(key + wsGUID))
	return base64.StdEncoding.EncodeToString(digest[:])
}

// ---------------------------------------------------------------------------
// WebSocket frame codec (RFC 6455 §5.2)
// ---------------------------------------------------------------------------

const (
	opContinuation byte = 0x0
	opText         byte = 0x1
	opBinary       byte = 0x2
	opClose        byte = 0x8
	opPing         byte = 0x9
	opPong         byte = 0xA
)

// wsFrame is one decoded RFC 6455 frame (already unmasked, if masked).
type wsFrame struct {
	fin     bool
	opcode  byte
	payload []byte
}

// buildFrame encodes one frame. Client->server frames MUST be masked;
// wsTransport never sends unmasked frames, but tests exercising the codec
// by itself (and the in-process fake server, whose server->client frames
// must NOT be masked) can turn it off.
func buildFrame(payload []byte, opcode byte, mask, fin bool) []byte {
	first := opcode & 0x0F
	if fin {
		first |= 0x80
	}
	out := []byte{first}

	length := len(payload)
	maskBit := byte(0x00)
	if mask {
		maskBit = 0x80
	}
	switch {
	case length < 126:
		out = append(out, maskBit|byte(length))
	case length < 1<<16:
		out = append(out, maskBit|126)
		out = binary.BigEndian.AppendUint16(out, uint16(length))
	default:
		out = append(out, maskBit|127)
		out = binary.BigEndian.AppendUint64(out, uint64(length))
	}

	if mask {
		key := make([]byte, 4)
		_, _ = rand.Read(key)
		out = append(out, key...)
		for i, b := range payload {
			out = append(out, b^key[i%4])
		}
	} else {
		out = append(out, payload...)
	}
	return out
}

// parseFrame parses one frame from the start of buf. It returns (nil, 0) if
// buf does not yet contain a complete frame (the caller should read more
// and retry); otherwise the frame and the number of bytes it consumed.
func parseFrame(buf []byte) (*wsFrame, int) {
	if len(buf) < 2 {
		return nil, 0
	}

	first, second := buf[0], buf[1]
	fin := first&0x80 != 0
	opcode := first & 0x0F
	masked := second&0x80 != 0
	length := int(second & 0x7F)

	offset := 2
	switch length {
	case 126:
		if len(buf) < offset+2 {
			return nil, 0
		}
		length = int(binary.BigEndian.Uint16(buf[offset:]))
		offset += 2
	case 127:
		if len(buf) < offset+8 {
			return nil, 0
		}
		length = int(binary.BigEndian.Uint64(buf[offset:]))
		offset += 8
	}

	var maskKey []byte
	if masked {
		if len(buf) < offset+4 {
			return nil, 0
		}
		maskKey = buf[offset : offset+4]
		offset += 4
	}

	if len(buf) < offset+length {
		return nil, 0
	}

	payload := make([]byte, length)
	copy(payload, buf[offset:offset+length])
	if masked {
		for i := range payload {
			payload[i] ^= maskKey[i%4]
		}
	}
	return &wsFrame{fin: fin, opcode: opcode, payload: payload}, offset + length
}

// ---------------------------------------------------------------------------
// socket-level connect + handshake
// ---------------------------------------------------------------------------

// wsConnect dials the endpoint and performs the HTTP Upgrade handshake. It
// returns the connection plus any bytes already read past the header block
// (the start of the first WebSocket frame, if the server was quick to send
// one).
func wsConnect(endpoint string) (net.Conn, []byte, error) {
	parts, err := url.Parse(endpoint)
	if err != nil || parts.Hostname() == "" {
		return nil, nil, &GraphQLError{Message: fmt.Sprintf("invalid WebSocket URL: %q", endpoint)}
	}
	port := parts.Port()
	if port == "" {
		if parts.Scheme == "wss" {
			port = "443"
		} else {
			port = "80"
		}
	}

	conn, err := net.DialTimeout("tcp", net.JoinHostPort(parts.Hostname(), port), 10*time.Second)
	if err != nil {
		return nil, nil, &GraphQLError{
			Message: fmt.Sprintf("cannot reach the bsdkrun daemon at %s — %v", endpoint, err),
		}
	}
	if parts.Scheme == "wss" {
		tlsConn := tls.Client(conn, &tls.Config{ServerName: parts.Hostname()})
		if err := tlsConn.Handshake(); err != nil {
			conn.Close()
			return nil, nil, &GraphQLError{
				Message: fmt.Sprintf("cannot reach the bsdkrun daemon at %s — %v", endpoint, err),
			}
		}
		conn = tlsConn
	}

	leftover, err := wsHandshake(conn, parts)
	if err != nil {
		conn.Close()
		return nil, nil, err
	}
	return conn, leftover, nil
}

func wsHandshake(conn net.Conn, parts *url.URL) ([]byte, error) {
	path := parts.Path
	if path == "" {
		path = "/"
	}
	if parts.RawQuery != "" {
		path += "?" + parts.RawQuery
	}
	key := makeWSKey()

	request := "GET " + path + " HTTP/1.1\r\n" +
		"Host: " + parts.Host + "\r\n" +
		"Upgrade: websocket\r\n" +
		"Connection: Upgrade\r\n" +
		"Sec-WebSocket-Key: " + key + "\r\n" +
		"Sec-WebSocket-Version: 13\r\n" +
		"Sec-WebSocket-Protocol: " + wsSubprotocol + "\r\n" +
		"\r\n"
	if _, err := conn.Write([]byte(request)); err != nil {
		return nil, &GraphQLError{Message: fmt.Sprintf("WebSocket handshake failed: %v", err)}
	}

	var buf []byte
	chunk := make([]byte, 4096)
	for !bytes.Contains(buf, []byte("\r\n\r\n")) {
		n, err := conn.Read(chunk)
		if n > 0 {
			buf = append(buf, chunk[:n]...)
		}
		if err != nil {
			return nil, &GraphQLError{
				Message: "the daemon closed the connection during the WebSocket handshake",
			}
		}
	}
	head, rest, _ := bytes.Cut(buf, []byte("\r\n\r\n"))

	lines := strings.Split(string(head), "\r\n")
	statusLine := lines[0]
	if !regexp.MustCompile(`^HTTP/1\.[01]\s+101\b`).MatchString(statusLine) {
		return nil, &GraphQLError{Message: fmt.Sprintf("WebSocket handshake failed: %q", statusLine)}
	}

	headers := map[string]string{}
	for _, line := range lines[1:] {
		if name, value, found := strings.Cut(line, ":"); found {
			headers[strings.ToLower(strings.TrimSpace(name))] = strings.TrimSpace(value)
		}
	}
	if headers["sec-websocket-accept"] != computeAccept(key) {
		return nil, &GraphQLError{
			Message: "WebSocket handshake failed: Sec-WebSocket-Accept did not match",
		}
	}
	return rest, nil
}

// ---------------------------------------------------------------------------
// subscription client
// ---------------------------------------------------------------------------

type wsSub struct {
	onNext     func(any)
	onError    func(error)
	onComplete func()
}

// wsTransport is one graphql-transport-ws connection, shared by every
// subscription a Client opens.
//
// Concurrency model: a single background reader goroutine (started lazily
// by the first subscribe) owns the socket's read side — it blocks in Read,
// decodes frames, and dispatches next/error/complete messages to the
// matching subscription's callbacks, all from that one goroutine. The
// public methods (subscribe, unsubscribe, close) only ever write to the
// socket and are safe to call from any goroutine; stateMu guards the shared
// map of subscriptions and the "are we connected/acked yet" bookkeeping,
// and writeMu serializes the actual socket writes so two goroutines writing
// at once can never interleave a frame. This keeps the rest of the SDK
// synchronous while still letting Client.Exec block a calling goroutine on
// a channel fed by the reader, and letting Client.Shell's callbacks fire
// from that same goroutine for live output.
type wsTransport struct {
	url   string
	token string

	stateMu sync.Mutex
	writeMu sync.Mutex
	conn    net.Conn
	subs    map[string]*wsSub
	pending []string
	acked   bool
	nextID  int
}

func newWSTransport(endpoint, token string) *wsTransport {
	return &wsTransport{url: endpoint, token: token, subs: map[string]*wsSub{}, nextID: 1}
}

func (t *wsTransport) ensureConnected() error {
	t.stateMu.Lock()
	if t.conn != nil {
		t.stateMu.Unlock()
		return nil
	}
	conn, leftover, err := wsConnect(t.url)
	if err != nil {
		t.stateMu.Unlock()
		return err
	}
	t.conn = conn
	t.acked = false
	t.pending = nil
	go t.readerLoop(conn, leftover)
	t.stateMu.Unlock()

	// The token travels in connection_init, not a header a real browser
	// could never set on a WS handshake anyway — keeping parity with the
	// other SDKs' behavior rather than relying on a header trick.
	init, _ := json.Marshal(map[string]any{
		"type":    "connection_init",
		"payload": map[string]any{"authorization": "Bearer " + t.token},
	})
	return t.sendRaw(string(init))
}

func (t *wsTransport) sendRaw(text string) error {
	t.stateMu.Lock()
	conn := t.conn
	t.stateMu.Unlock()
	if conn == nil {
		return &GraphQLError{Message: "the WebSocket is not connected"}
	}
	frame := buildFrame([]byte(text), opText, true, true)
	t.writeMu.Lock()
	defer t.writeMu.Unlock()
	if _, err := conn.Write(frame); err != nil {
		return &GraphQLError{Message: fmt.Sprintf("cannot write to the daemon — %v", err)}
	}
	return nil
}

// close closes the socket. Idempotent.
func (t *wsTransport) close() {
	t.stateMu.Lock()
	conn := t.conn
	t.conn = nil
	t.subs = map[string]*wsSub{}
	t.pending = nil
	t.acked = false
	t.stateMu.Unlock()
	if conn != nil {
		conn.Close()
	}
}

// subscribe starts a subscription and returns its id (pass to unsubscribe).
func (t *wsTransport) subscribe(
	query string,
	variables map[string]any,
	onNext func(any),
	onError func(error),
	onComplete func(),
) (string, error) {
	if err := t.ensureConnected(); err != nil {
		return "", err
	}
	if variables == nil {
		variables = map[string]any{}
	}
	if onError == nil {
		onError = func(error) {}
	}
	if onComplete == nil {
		onComplete = func() {}
	}

	t.stateMu.Lock()
	subID := strconv.Itoa(t.nextID)
	t.nextID++
	t.subs[subID] = &wsSub{onNext: onNext, onError: onError, onComplete: onComplete}
	message, _ := json.Marshal(map[string]any{
		"id":      subID,
		"type":    "subscribe",
		"payload": map[string]any{"query": query, "variables": variables},
	})
	acked := t.acked
	if !acked {
		// Flushed once connection_ack arrives (see dispatchText).
		t.pending = append(t.pending, string(message))
	}
	t.stateMu.Unlock()

	if acked {
		if err := t.sendRaw(string(message)); err != nil {
			return "", err
		}
	}
	return subID, nil
}

func (t *wsTransport) unsubscribe(subID string) {
	t.stateMu.Lock()
	if _, ok := t.subs[subID]; !ok {
		t.stateMu.Unlock()
		return
	}
	delete(t.subs, subID)
	remaining := len(t.subs)
	conn := t.conn
	t.stateMu.Unlock()

	if conn != nil {
		message, _ := json.Marshal(map[string]any{"id": subID, "type": "complete"})
		_ = t.sendRaw(string(message)) // best effort — the socket may already be gone
	}
	// Drop the socket once nothing is using it, so a later subscribe
	// reconnects fresh rather than talking to a stale connection.
	if remaining == 0 {
		t.close()
	}
}

// -- reader goroutine -------------------------------------------------------

func (t *wsTransport) readerLoop(conn net.Conn, leftover []byte) {
	buf := append([]byte(nil), leftover...)
	// Minimal fragmentation support: accumulate continuation frames into
	// one payload and dispatch on the final (FIN) frame. In practice
	// async-graphql's WS implementation (axum/tungstenite underneath)
	// sends each graphql-transport-ws message as a single unfragmented
	// text frame, so this path is exercised mainly by the frame-codec
	// unit tests, not real daemon traffic.
	var frag []byte
	chunk := make([]byte, 4096)

loop:
	for {
		frame, consumed := parseFrame(buf)
		if frame == nil {
			n, err := conn.Read(chunk)
			if n > 0 {
				buf = append(buf, chunk[:n]...)
			}
			if err != nil && n == 0 {
				break
			}
			continue
		}
		buf = buf[consumed:]

		switch frame.opcode {
		case opContinuation:
			frag = append(frag, frame.payload...)
			if frame.fin {
				t.dispatchText(frag)
				frag = nil
			}
		case opText, opBinary:
			if !frame.fin {
				frag = append([]byte(nil), frame.payload...)
				continue
			}
			t.dispatchText(frame.payload)
		case opPing:
			pong := buildFrame(frame.payload, opPong, true, true)
			t.writeMu.Lock()
			_, _ = conn.Write(pong)
			t.writeMu.Unlock()
		case opClose:
			break loop
		}
		// opPong (and anything unrecognized): nothing to do.
	}
	t.onSocketClosed(conn)
}

func (t *wsTransport) dispatchText(payload []byte) {
	var msg struct {
		Type    string          `json:"type"`
		ID      string          `json:"id"`
		Payload json.RawMessage `json:"payload"`
	}
	if err := json.Unmarshal(payload, &msg); err != nil {
		return
	}

	switch msg.Type {
	case "connection_ack":
		t.stateMu.Lock()
		t.acked = true
		pending := t.pending
		t.pending = nil
		t.stateMu.Unlock()
		for _, text := range pending {
			_ = t.sendRaw(text)
		}

	case "ping":
		pong, _ := json.Marshal(map[string]any{"type": "pong"})
		_ = t.sendRaw(string(pong))

	case "pong":
		// nothing to do

	case "next":
		t.stateMu.Lock()
		sub := t.subs[msg.ID]
		t.stateMu.Unlock()
		if sub != nil {
			var body struct {
				Data any `json:"data"`
			}
			_ = json.Unmarshal(msg.Payload, &body)
			sub.onNext(body.Data)
		}

	case "error":
		t.stateMu.Lock()
		sub := t.subs[msg.ID]
		delete(t.subs, msg.ID)
		t.stateMu.Unlock()
		if sub != nil {
			var errs []map[string]any
			detail := string(msg.Payload)
			if err := json.Unmarshal(msg.Payload, &errs); err == nil {
				parts := make([]string, 0, len(errs))
				for _, e := range errs {
					parts = append(parts, asString(e["message"]))
				}
				detail = strings.Join(parts, "; ")
			}
			sub.onError(&GraphQLError{Message: detail})
		}

	case "complete":
		t.stateMu.Lock()
		sub := t.subs[msg.ID]
		delete(t.subs, msg.ID)
		t.stateMu.Unlock()
		if sub != nil {
			sub.onComplete()
		}
	}
}

func (t *wsTransport) onSocketClosed(conn net.Conn) {
	conn.Close()
	t.stateMu.Lock()
	if t.conn != conn && t.conn != nil {
		// A newer connection replaced this one; its reader owns the state.
		t.stateMu.Unlock()
		return
	}
	wasAcked := t.acked
	subs := make([]*wsSub, 0, len(t.subs))
	for _, sub := range t.subs {
		subs = append(subs, sub)
	}
	t.subs = map[string]*wsSub{}
	t.pending = nil
	t.conn = nil
	t.acked = false
	t.stateMu.Unlock()

	// An unacked close means the daemon rejected connection_init (a bad
	// token) and hung up before ever getting to acknowledge it; an acked
	// close is just the connection going away later on.
	var err error = &AuthError{}
	if wasAcked {
		err = &GraphQLError{Message: "the connection to the daemon was closed"}
	}
	for _, sub := range subs {
		sub.onError(err)
	}
}
