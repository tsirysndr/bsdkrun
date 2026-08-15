# frozen_string_literal: true

require "json"

module Bsdkrun
  # A stored cache entry, as +cache ls+ reports it.
  CacheEntry = Struct.new(:key, :path, :compression, :size, :created, :digest, keyword_init: true) do
    def self.from(row)
      new(
        key: row["key"].to_s,
        path: row["path"].to_s,
        compression: row["compression"].to_s,
        size: row["size"].to_i,
        created: row["created"].to_i,
        digest: row["digest"].to_s
      )
    end
  end

  # What a restore did. A miss is not an error — check +restored+.
  RestoreResult = Struct.new(
    :restored, :requested_key, :key, :path, :size, :compression, :created,
    keyword_init: true
  )

  # Save and restore guest directories under a key, reached as {Sandbox#cache}.
  #
  # Entries are keyed, so a rebuild can pick up where the last one left off:
  #
  #   hit = sbx.cache.restore(key: key, restore_keys: ["deps-"])
  #   unless hit.restored
  #     sbx.exec(["npm", "ci"])
  #     sbx.cache.save("/app/node_modules", key: key)
  #   end
  #
  # Where entries live — host disk or S3 — is host configuration, not an SDK
  # concern: set +BSDKRUN_CACHE_BACKEND+ / +BSDKRUN_CACHE_S3_*+, or write
  # +~/.config/bsdkrun/cache.toml+.
  class Cache
    # @param id [String] the machine's id.
    def initialize(id)
      @id = id
    end

    # Archive the guest directory at +path+ under +key+.
    #
    # @param path [String] absolute path in the guest.
    # @param key [String] key to store under.
    # @param compression [String] gzip (default), zstd, estargz or none.
    # @param force [Boolean] replace an entry that already has this key.
    # @return [CacheEntry]
    def save(path, key:, compression: "gzip", force: false)
      args = ["cache", "save", "#{@id}:#{path}", "--key", key, "--json"]
      args += ["--compression", compression] unless compression == "gzip"
      args << "--force" if force
      CacheEntry.from(json(args, "bsdkrun cache save"))
    end

    # Restore a stored tree.
    #
    # @param key [String]
    # @param path [String, nil] defaults to where the entry was saved from.
    # @param restore_keys [Array<String>] prefixes tried in order on a miss.
    # @return [RestoreResult]
    def restore(key:, path: nil, restore_keys: [])
      target = path ? "#{@id}:#{path}" : @id
      args = ["cache", "restore", target, "--key", key, "--json"]
      args += ["--restore-keys", *restore_keys] unless restore_keys.empty?
      row = json(args, "bsdkrun cache restore")
      RestoreResult.new(
        restored: !!row["restored"],
        requested_key: row["requested_key"].to_s,
        key: row["key"],
        path: row["path"],
        size: row["size"],
        compression: row["compression"],
        created: row["created"]
      )
    end

    private

    def json(args, label)
      out = Process.run!(args, label: label).stdout
      JSON.parse(out.strip.empty? ? "{}" : out)
    end
  end

  # Host-level cache operations, mirroring {Bsdkrun.volumes}.
  module Caches
    module_function

    # @return [Array<CacheEntry>] every stored entry, newest first.
    def ls
      out = Process.run!(["cache", "ls", "--json"], label: "bsdkrun cache ls").stdout
      JSON.parse(out.strip.empty? ? "[]" : out).map { |row| CacheEntry.from(row) }
    end

    # Remove entries by key, or every one with <tt>all: true</tt>.
    # @return [void]
    def rm(keys = [], all: false)
      args = ["cache", "rm"]
      if all
        args << "--all"
      else
        args += Array(keys)
      end
      Process.run!(args, label: "bsdkrun cache rm")
      nil
    end
  end
end
