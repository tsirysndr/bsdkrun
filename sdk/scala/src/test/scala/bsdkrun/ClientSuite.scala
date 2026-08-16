package bsdkrun

class ClientSuite extends munit.FunSuite:

  // These mirror `web/src/lib/connection.ts`'s `normalizeUrl` exactly; the SDK
  // and the browser client have to agree on what a user-typed host means.
  test("normalizeUrl fills in the scheme and the /graphql path") {
    assertEquals(Client.normalizeUrl("localhost:8080"), "http://localhost:8080/graphql")
    assertEquals(Client.normalizeUrl("http://host/"), "http://host/graphql")
    assertEquals(Client.normalizeUrl("https://host/graphql"), "https://host/graphql")
    assertEquals(Client.normalizeUrl("  https://host///  "), "https://host/graphql")
    assertEquals(Client.normalizeUrl("HTTP://host"), "HTTP://host/graphql")
    assertEquals(Client.normalizeUrl(""), "")
  }

  test("wsUrl swaps the scheme and appends /ws") {
    assertEquals(Client.wsUrl("http://host/graphql"), "ws://host/graphql/ws")
    assertEquals(Client.wsUrl("https://host/graphql"), "wss://host/graphql/ws")
    assertEquals(Client.wsUrl("https://host/graphql/"), "wss://host/graphql/ws")
  }

  // A URL without a token is an error, not a silent unauthenticated fallback —
  // the daemon would just 401 and the reason would be invisible.
  test("fromEnv needs both the URL and the token") {
    val missingUrl = Client.fromEnv(Map.empty).swap.getOrElse(fail("expected an error"))
    assert(missingUrl.message.contains(Client.UrlEnv), missingUrl.message)

    val missingToken = Client
      .fromEnv(Map(Client.UrlEnv -> "http://host"))
      .swap
      .getOrElse(fail("expected an error"))
    assert(missingToken.message.contains(Client.TokenEnv), missingToken.message)

    val ok = Client.fromEnv(Map(Client.UrlEnv -> "host:9000", Client.TokenEnv -> "t"))
    assertEquals(ok.map(_.url), Right("http://host:9000/graphql"))
  }

  test("a blank URL or token counts as unset") {
    assert(Client.fromEnv(Map(Client.UrlEnv -> "   ")).isLeft)
    assert(Client.fromEnv(Map(Client.UrlEnv -> "h", Client.TokenEnv -> "")).isLeft)
  }

  test("a GraphQL response yields its data") {
    val json = ujson.read("""{"data":{"machines":[]}}""")
    assertEquals(Client.dataOrError(json).map(_.render()), Right("""{"machines":[]}"""))
  }

  // An UNAUTHENTICATED code has to become an auth error specifically: a caller
  // retrying a generic failure forever on a bad token is the bug this prevents.
  test("an UNAUTHENTICATED error is an auth failure, not a generic one") {
    val json = ujson.read(
      """{"errors":[{"message":"nope","extensions":{"code":"UNAUTHENTICATED"}}]}"""
    )
    Client.dataOrError(json) match
      case Left(BsdkrunError.Auth(detail)) => assertEquals(detail, "nope")
      case other                           => fail(s"expected an auth error, got $other")
  }

  test("any other GraphQL error keeps its code") {
    val json = ujson.read(
      """{"errors":[{"message":"bad id","extensions":{"code":"INVALID_ARGUMENT"}}]}"""
    )
    Client.dataOrError(json) match
      case Left(BsdkrunError.GraphQL(detail, code)) =>
        assertEquals(detail, "bad id")
        assertEquals(code, Some("INVALID_ARGUMENT"))
      case other => fail(s"expected a graphql error, got $other")
  }

  test("a response with neither data nor errors is a failure") {
    assert(Client.dataOrError(ujson.read("{}")).isLeft)
  }
