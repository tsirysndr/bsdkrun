# frozen_string_literal: true

module Bsdkrun
  # Files in a running sandbox, reached as {Sandbox#fs}.
  #
  # Every call goes through the guest's exec agent, so the sandbox has to be
  # running — there is no offline write.
  #
  # @example
  #   box.fs.write_file("/app/main.py", "print('hi')")
  #   box.fs.read_text("/app/out.json")
  #   box.fs.upload("./src", "/app/src")
  #   box.fs.download("/app/dist", "./dist", recursive: true)
  class FileSystem
    # @param id [String] the machine's id.
    def initialize(id)
      @id = id
    end

    # Write +data+ to +path+ in the guest, creating parent directories.
    #
    # @param path [String] absolute path in the guest.
    # @param data [String] text or binary content.
    # @return [void]
    # @raise [FileTransferFailed]
    def write_file(path, data)
      res = Process.run(["cp", "-", "#{@id}:#{path}"], stdin: data, binary: true)
      check!(res, path)
      nil
    end

    # Read +path+ from the guest as bytes (ASCII-8BIT).
    #
    # @param path [String]
    # @return [String] binary string.
    # @raise [FileTransferFailed]
    def read_file(path)
      res = Process.run(["cp", "#{@id}:#{path}", "-"], binary: true)
      check!(res, path)
      res.stdout
    end

    # Read +path+ from the guest and tag it with +encoding+.
    #
    # @param path [String]
    # @param encoding [String]
    # @return [String]
    def read_text(path, encoding: "UTF-8")
      read_file(path).force_encoding(encoding)
    end

    # Copy a host file or directory into the guest.
    #
    # A directory's *contents* land in +remote_path+, so
    # <tt>upload("./src", "/app/src")</tt> leaves the guest's +/app/src+ holding
    # what +./src+ holds. Whether it recurses is decided by looking at the local
    # path, so callers do not have to say which kind of thing it is.
    #
    # @param local_path [String]
    # @param remote_path [String]
    # @return [void]
    # @raise [FileTransferFailed]
    def upload(local_path, remote_path)
      unless File.exist?(local_path)
        raise FileTransferFailed.new("cannot upload #{local_path}: no such file or directory",
                                     local_path)
      end
      args = ["cp"]
      args << "-r" if File.directory?(local_path)
      args += [local_path.to_s, "#{@id}:#{remote_path}"]
      check!(Process.run(args), local_path)
      nil
    end

    # Copy a file or directory out of the guest onto the host.
    #
    # Pass <tt>recursive: true</tt> for a directory; unlike {#upload} it cannot
    # be detected here, because the path lives in the guest and answering would
    # cost an extra round trip.
    #
    # @param remote_path [String]
    # @param local_path [String]
    # @param recursive [Boolean]
    # @return [void]
    # @raise [FileTransferFailed]
    def download(remote_path, local_path, recursive: false)
      args = ["cp"]
      args << "-r" if recursive
      args += ["#{@id}:#{remote_path}", local_path.to_s]
      check!(Process.run(args), remote_path)
      nil
    end

    private

    def check!(res, path)
      return if res.exit_code.zero?

      # The CLI already explains these well; strip its "Error: " prefix.
      text = res.stderr.to_s.strip.sub(/\AError:\s*/, "")
      text = "file transfer failed for #{path}" if text.empty?
      raise FileTransferFailed.new(text, path)
    end
  end
end
