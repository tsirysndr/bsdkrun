// Where the daemon is and how to authenticate to it.
//
// The web app is served from anywhere and talks to a bsdkrun daemon over the
// network, so — unlike the desktop app, which finds a local binary — it cannot
// discover its backend. The user supplies the endpoint and token on first run
// and can change them later from Settings.
//
// The token is a bearer credential kept in localStorage. That is the tradeoff
// the daemon's design already makes: it authenticates the holder of a token,
// with no cookie or session for a browser to hold on your behalf. Anything
// that can run script on this origin can therefore read it, so serve the app
// from an origin you trust.

const STORAGE_KEY = "bsdkrun.connection";

export interface Connection {
  /** Full URL of the GraphQL endpoint, e.g. http://localhost:50052/graphql */
  url: string;
  token: string;
}

let cached: Connection | null | undefined;

export function getConnection(): Connection | null {
  if (cached !== undefined) return cached;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? (JSON.parse(raw) as Connection) : null;
    cached = parsed?.url && parsed?.token ? parsed : null;
  } catch {
    cached = null;
  }
  return cached;
}

export function setConnection(conn: Connection) {
  cached = { url: normalizeUrl(conn.url), token: conn.token.trim() };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(cached));
}

export function clearConnection() {
  cached = null;
  localStorage.removeItem(STORAGE_KEY);
}

/**
 * Accept what people actually paste and turn it into the endpoint URL.
 *
 * The daemon prints `http://127.0.0.1:50052` in its banner, so a bare origin is
 * the most likely thing to arrive here; appending `/graphql` beats making the
 * first-run experience a guessing game. A scheme is assumed when missing,
 * because `localhost:50052` is what people type.
 */
export function normalizeUrl(input: string): string {
  let s = input.trim();
  if (!s) return s;
  if (!/^https?:\/\//i.test(s)) s = `http://${s}`;
  s = s.replace(/\/+$/, "");
  if (!/\/graphql$/i.test(s)) s = `${s}/graphql`;
  return s;
}

/** The default a first-run user is most likely to want. */
export const DEFAULT_URL = "http://localhost:50052/graphql";
