package deploy

// Deno Deploy: deployctl reads DENO_DEPLOY_TOKEN from the environment
// and ships the module graph directly.
func DenoDeploy() Target {
	return Target{
		Platform: "deno-deploy",
		Secret:   "DENO_DEPLOY_TOKEN",
		Command:  `deployctl deploy --prod`,
	}
}
