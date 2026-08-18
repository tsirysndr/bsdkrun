package deploy

// Railway: RAILWAY_TOKEN authenticates the CLI directly; `up` builds
// and deploys the linked project.
func Railway() Target {
	return Target{
		Platform: "railway",
		Secret:   "RAILWAY_TOKEN",
		Command:  `railway up --detach`,
	}
}
