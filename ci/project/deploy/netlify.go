package deploy

// Netlify: NETLIFY_AUTH_TOKEN is the CLI's own auth variable.
func Netlify() Target {
	return Target{
		Platform: "netlify",
		Secret:   "NETLIFY_AUTH_TOKEN",
		Command:  `npx netlify-cli deploy --prod`,
	}
}
