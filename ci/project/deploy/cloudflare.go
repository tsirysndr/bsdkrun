package deploy

// Cloudflare Workers: wrangler reads CLOUDFLARE_API_TOKEN from the
// environment; npx keeps the CLI out of the image.
func Cloudflare() Target {
	return Target{
		Platform: "cloudflare",
		Secret:   "CLOUDFLARE_API_TOKEN",
		Command:  `npx wrangler deploy`,
	}
}
