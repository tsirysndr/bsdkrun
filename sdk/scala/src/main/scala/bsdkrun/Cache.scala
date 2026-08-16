package bsdkrun

import ujson.Value

/** An archive format a cache entry can be stored in. */
enum Compression(val flag: String):
  case Gzip extends Compression("gzip")
  case Zstd extends Compression("zstd")
  case Estargz extends Compression("estargz")
  case Uncompressed extends Compression("none")

/** A stored cache entry, as `cache ls` reports it. */
final case class CacheEntry(
    key: String,
    /** Guest path the tree came from. */
    path: String,
    compression: String,
    /** Archive size in bytes. */
    size: Long,
    /** Unix seconds when it was saved. */
    created: Long,
    /** `sha256:…` over the archive. */
    digest: String
)

object CacheEntry:
  private[bsdkrun] def decode(v: Value): CacheEntry = CacheEntry(
    key = Types.str(v, "key"),
    path = Types.str(v, "path"),
    compression = Types.str(v, "compression"),
    size = Types.optLong(v, "size").getOrElse(0L),
    created = Types.optLong(v, "created").getOrElse(0L),
    digest = Types.str(v, "digest")
  )

/** What a restore did. A miss is **not** an error — check [[restored]]. */
final case class RestoreResult(
    restored: Boolean,
    /** The key asked for. */
    requestedKey: String,
    /** The entry actually used. Differs from [[requestedKey]] when a restore-key
      * prefix matched, and is `None` on a miss.
      */
    key: Option[String],
    /** Guest path it was restored into. */
    path: Option[String],
    size: Option[Long],
    compression: Option[String],
    created: Option[Long]
)

object RestoreResult:
  private[bsdkrun] def decode(v: Value): RestoreResult = RestoreResult(
    restored = Types.bool(v, "restored"),
    requestedKey = Types.str(v, "requested_key"),
    key = Types.optStr(v, "key"),
    path = Types.optStr(v, "path"),
    size = Types.optLong(v, "size"),
    compression = Types.optStr(v, "compression"),
    created = Types.optLong(v, "created")
  )

/** Cached guest directories for one sandbox, reached as [[Sandbox.cache]].
  *
  * Entries are keyed, so a rebuild can pick up where the last one left off:
  *
  * {{{
  * for
  *   hit <- sbx.cache.restore(key, restoreKeys = Seq("deps-"))
  *   _   <- if hit.restored then Right(()) else
  *            sbx.exec("npm", "ci").flatMap(_ => sbx.cache.save("/app/node_modules", key)).map(_ => ())
  * yield ()
  * }}}
  *
  * Where entries live — host disk or S3 — is host configuration, not an SDK
  * concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
  * `~/.config/bsdkrun/cache.toml`.
  */
final class Cache private[bsdkrun] (id: String):

  /** Archive the guest directory at `path` under `key`. */
  def save(
      path: String,
      key: String,
      compression: Compression = Compression.Gzip,
      force: Boolean = false
  ): Either[BsdkrunError, CacheEntry] =
    val args = Seq("cache", "save", s"$id:$path", "--key", key, "--json") ++
      (if compression == Compression.Gzip then Nil else Seq("--compression", compression.flag)) ++
      (if force then Seq("--force") else Nil)
    Cache.json(args, "bsdkrun cache save").map(CacheEntry.decode)

  /** Restore a stored tree.
    *
    * `path` defaults to the directory the entry was saved from. `restoreKeys`
    * are prefixes tried in order when `key` misses; within a prefix the newest
    * matching entry wins. A miss comes back as `Right` with `restored = false`.
    */
  def restore(
      key: String,
      path: Option[String] = None,
      restoreKeys: Seq[String] = Seq.empty
  ): Either[BsdkrunError, RestoreResult] =
    val target = path.map(p => s"$id:$p").getOrElse(id)
    val args = Seq("cache", "restore", target, "--key", key, "--json") ++
      (if restoreKeys.isEmpty then Nil else "--restore-keys" +: restoreKeys)
    Cache.json(args, "bsdkrun cache restore").map(RestoreResult.decode)

object Cache:

  /** Every stored cache entry, newest first. */
  def list(): Either[BsdkrunError, Seq[CacheEntry]] =
    Proc
      .runChecked(Seq("cache", "ls", "--json"), "bsdkrun cache ls")
      .flatMap(res => Types.rows(res.stdout, "bsdkrun cache ls", CacheEntry.decode))

  /** Remove entries by key. */
  def remove(keys: Seq[String]): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("cache", "rm") ++ keys, "bsdkrun cache rm").map(_ => ())

  /** Remove every stored entry. */
  def removeAll(): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("cache", "rm", "--all"), "bsdkrun cache rm").map(_ => ())

  private def json(args: Seq[String], label: String): Either[BsdkrunError, Value] =
    Proc.runChecked(args, label).flatMap(res => Types.one(res.stdout, label))
