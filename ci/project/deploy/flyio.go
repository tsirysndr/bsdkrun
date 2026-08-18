package deploy

// Fly.io: FLY_API_TOKEN is flyctl's own auth variable; --remote-only
// builds on Fly's builders, so the guest needs no Docker.
func Fly() Target {
	return Target{
		Platform: "fly",
		Secret:   "FLY_API_TOKEN",
		Command:  `flyctl deploy --remote-only`,
	}
}
