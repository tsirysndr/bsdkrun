package deploy

// Koyeb: KOYEB_TOKEN authenticates the CLI.
func Koyeb() Target {
	return Target{
		Platform: "koyeb",
		Secret:   "KOYEB_TOKEN",
		Command:  `koyeb deploy`,
	}
}
