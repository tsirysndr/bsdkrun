Rails.application.routes.draw do
  get "/hello" => "hello#index", :as => :hello
  # Upstream serves only /hello; the root route is added so the e2e check (a
  # plain GET /) exercises the application rather than the welcome page.
  root "hello#index"
end
