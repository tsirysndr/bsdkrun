//// Host-level image operations.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import bsdkrun/types.{type ImageInfo}
import gleam/result

/// List downloaded OCI images.
pub fn list() -> Result(List(ImageInfo), Error) {
  use out <- result.try(cli.checked(
    ["images", "--json"],
    "bsdkrun images",
    cli.options(),
  ))

  types.decode_rows(out.stdout, "bsdkrun images", types.image_info_decoder())
}
