// wisp ships its own request simulator (wisp/simulate), so the handler is
// tested with zero new dependencies: build the same requests the unikernel
// e2e sends, assert the same bodies. `gleam test` runs this main; a failed
// `let assert` panics, which is the failure signal.
import gleam/http
import wisp/simulate
import wisp_demo

pub fn main() -> Nil {
  let home = wisp_demo.handle_request(simulate.request(http.Get, "/"))
  let assert 200 = home.status
  let assert "Hello from Gleam on Unikraft!\n" = simulate.read_body(home)

  let info = wisp_demo.handle_request(simulate.request(http.Get, "/info"))
  let assert 200 = info.status

  let missing = wisp_demo.handle_request(simulate.request(http.Get, "/nope"))
  let assert 404 = missing.status

  Nil
}
