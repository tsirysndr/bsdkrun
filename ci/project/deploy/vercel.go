package deploy

// Vercel: the CLI wants the token as a flag rather than the
// environment; --yes skips the interactive link step.
func Vercel() Target {
	return Target{
		Platform: "vercel",
		Secret:   "VERCEL_TOKEN",
		Command:  `npx vercel deploy --prod --yes --token "$VERCEL_TOKEN"`,
	}
}
