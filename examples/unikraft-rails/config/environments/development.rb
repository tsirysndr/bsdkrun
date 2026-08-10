Rails.application.configure do
  # The guest answers on whatever address the host forwards to it; the
  # development-mode host allowlist would reject those requests with a 403.
  config.hosts.clear
end
