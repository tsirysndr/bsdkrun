import gleam/erlang/atom.{type Atom}
import gleam/erlang/charlist.{type Charlist}
import gleam/erlang/process
import gleam/http/request
import gleam/json
import mist
import wisp
import wisp/wisp_mist

pub fn main() -> Nil {
  wisp.configure_logger()

  // Signed cookies and flash messages are keyed off this. Nothing here uses
  // either, and a unikernel has no environment to read a real secret from, so
  // a fresh random value per boot is the honest choice -- it is what makes the
  // key unusable for anything that has to survive a restart.
  let secret_key_base = wisp.random_string(64)

  let assert Ok(_) =
    wisp_mist.handler(handle_request, secret_key_base)
    |> mist.new
    |> mist.bind("0.0.0.0")
    |> mist.port(3000)
    |> mist.start

  wisp.log_info("Gleam listening on port 3000")

  // The unikernel runs exactly one program: when main returns, the guest
  // powers off. mist's listener lives in a supervisor of its own, so this
  // process has nothing left to do but stay alive.
  process.sleep_forever()
}

pub fn handle_request(req: wisp.Request) -> wisp.Response {
  case request.path_segments(req) {
    [] ->
      wisp.ok()
      |> wisp.string_body("Hello from Gleam on Unikraft!\n")

    ["info"] ->
      json.object([
        #("runtime", json.string("gleam")),
        #("gleam", json.string(gleam_version)),
        #("otp", json.string(system_info("otp_release"))),
        #("erts", json.string(system_info("version"))),
      ])
      |> json.to_string
      |> wisp.json_response(200)

    _ -> wisp.not_found()
  }
}

// Gleam erases its own version at compile time -- there is no runtime to ask --
// so this is the one value that has to be written down. It is the compiler in
// the Dockerfile's build stage.
const gleam_version = "1.18.0"

// `erlang:system_info/1` answers both of the keys used above with a charlist,
// which is why one external is enough for both. Reaching for Erlang like this
// is ordinary Gleam, not a workaround for the unikernel.
@external(erlang, "erlang", "system_info")
fn erlang_system_info(key: Atom) -> Charlist

fn system_info(key: String) -> String {
  key
  |> atom.create
  |> erlang_system_info
  |> charlist.to_string
}
