package deploy

// Heroku: HEROKU_API_KEY is the platform's standard CLI auth
// variable; builds:create ships the source as a build.
func Heroku() Target {
	return Target{
		Platform: "heroku",
		Secret:   "HEROKU_API_KEY",
		Command:  `heroku builds:create`,
	}
}
