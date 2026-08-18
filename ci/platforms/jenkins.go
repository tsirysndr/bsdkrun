package platforms

// Jenkins: the Jenkinsfile at the repository root — the declarative dialect
// only, and that is a scope decision, not a shortcut. A *scripted* pipeline
// (`node { ... }`) is an arbitrary Groovy program executing against Jenkins'
// CPS runtime; nothing short of embedding Jenkins runs one faithfully, so a
// scripted file gets a clear refusal instead of a wrong translation. A
// *declarative* pipeline is a rigid block skeleton, and parsing that needs a
// small structural tokenizer (comments, strings, braces, statements), not a
// Groovy implementation.
//
// What translates: the pipeline (or per-stage) docker agent image, `agent
// any` on the default image, `environment { K = 'literal' }` at pipeline
// and stage level, stages in order — `parallel` stages run serially, like
// every parallel construct here — and `sh` steps in all three spellings
// (`sh 'x'`, `sh "x"`, `sh(script: 'x')`), plus `echo`. `checkout scm`
// dissolves into the clone that already happened. Everything else in
// `steps` becomes a visible skip; `post`, `options`, `triggers`, `when` and
// `tools` are ignored; environment values that are Groovy expressions
// (`credentials(...)`, string interpolation of calls) are dropped rather
// than mistranslated. Agent labels naming windows or macos skip the job.

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func detectJenkins(root string) bool {
	return fileExists(filepath.Join(root, "Jenkinsfile"))
}

func loadJenkins(root string, repo Repo) ([]Job, error) {
	data, err := os.ReadFile(filepath.Join(root, "Jenkinsfile"))
	if err != nil {
		return nil, err
	}
	nodes, err := gvParse(string(data))
	if err != nil {
		return nil, fmt.Errorf("Jenkinsfile: %w", err)
	}

	var pipeline *gvNode
	for i := range nodes {
		switch nodes[i].name {
		case "pipeline":
			pipeline = &nodes[i]
		case "node", "stage", "properties":
			return nil, fmt.Errorf(
				"this Jenkinsfile is a scripted pipeline (a Groovy program); only " +
					"declarative pipelines (`pipeline { ... }`) translate locally")
		}
	}
	if pipeline == nil {
		return nil, fmt.Errorf("no `pipeline { ... }` block in the Jenkinsfile")
	}

	job := Job{Name: "pipeline", Env: map[string]string{}}
	var divergent []string

	for _, n := range pipeline.block {
		switch n.name {
		case "agent":
			img, skip := gvAgent(n)
			job.Image = img
			if skip != "" {
				job.SkipReason = skip
			}
		case "environment":
			for k, v := range gvEnv(n) {
				job.Env[k] = v
			}
		case "stages":
			gvStages(n, &job, &divergent)
		}
	}
	if len(divergent) > 0 {
		note := Step{
			Name: "per-stage agents (not supported)",
			Command: fmt.Sprintf(
				`echo "stages declaring their own agents run on the pipeline image here: %s"`,
				strings.Join(divergent, ", ")),
		}
		job.Steps = append([]Step{note}, job.Steps...)
	}
	if len(job.Steps) == 0 {
		return nil, fmt.Errorf("the Jenkinsfile's stages contain no translatable steps")
	}
	return []Job{job}, nil
}

// gvStages walks stages, including `parallel` and nested `stages` groups.
func gvStages(stages gvNode, job *Job, divergent *[]string) {
	for _, st := range stages.block {
		if st.name != "stage" {
			continue
		}
		stageName := "stage"
		if len(st.args) > 0 {
			stageName = st.args[0]
		}
		stageEnv := map[string]string{}
		var steps *gvNode
		for i := range st.block {
			n := &st.block[i]
			switch n.name {
			case "agent":
				if img, _ := gvAgent(*n); img != "" && img != job.Image {
					*divergent = append(*divergent, fmt.Sprintf("%s (%s)", stageName, img))
				}
			case "environment":
				for k, v := range gvEnv(*n) {
					stageEnv[k] = v
				}
			case "steps":
				steps = n
			case "parallel", "stages":
				gvStages(*n, job, divergent)
			}
		}
		if steps == nil {
			continue
		}
		var commands []string
		for _, s := range steps.block {
			switch s.name {
			case "sh":
				// All three spellings: sh 'x', sh "x", sh(script: 'x') —
				// the command is the first quoted string either way.
				if v, ok := gvFirstString(s); ok {
					commands = append(commands, v)
				}
			case "echo":
				if v, ok := gvFirstString(s); ok {
					commands = append(commands, "echo "+shellQuote(v))
				}
			case "checkout":
				// `checkout scm` is the clone that already happened.
			default:
				commands = append(commands, fmt.Sprintf(
					`echo "skipped step %q — only sh/echo translate locally"`, s.name))
			}
		}
		if len(commands) == 0 {
			continue
		}
		env := stageEnv
		if len(env) == 0 {
			env = nil
		}
		job.Steps = append(job.Steps, Step{
			Name:    stageName,
			Command: strings.Join(commands, "\n"),
			Env:     env,
		})
	}
}

// gvAgent reads `agent any|none`, `agent { docker 'img' }`,
// `agent { docker { image 'img' } }`, `agent { label 'x' }`.
func gvAgent(n gvNode) (image, skip string) {
	for _, a := range n.args {
		if s := linuxOnly(a); s != "" {
			skip = s
		}
	}
	for _, c := range n.block {
		switch c.name {
		case "docker", "dockerfile":
			if len(c.args) > 0 {
				image = c.args[0]
			}
			for _, cc := range c.block {
				if cc.name == "image" && len(cc.args) > 0 {
					image = cc.args[0]
				}
			}
		case "label", "node":
			for _, a := range c.args {
				if s := linuxOnly(a); s != "" {
					skip = s
				}
			}
		}
	}
	return image, skip
}

// gvEnv keeps only literal assignments; a Groovy expression on the right —
// credentials(...), a call, arithmetic — is not a value this runner can
// truthfully provide.
func gvEnv(n gvNode) map[string]string {
	out := map[string]string{}
	for _, c := range n.block {
		if len(c.args) >= 2 && c.args[0] == "=" && c.literal[1] {
			out[c.name] = c.args[1]
		}
	}
	return out
}

func gvFirstString(n gvNode) (string, bool) {
	for i, a := range n.args {
		if n.literal[i] {
			return a, true
		}
	}
	return "", false
}

func shellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

// --- the structural parser ---------------------------------------------------

// gvNode is one statement: a leading identifier, its argument tokens on the
// same line (strings unquoted; `literal` marks which were quoted strings),
// and a nested block when `{ ... }` followed.
type gvNode struct {
	name    string
	args    []string
	literal []bool
	block   []gvNode
}

type gvToken struct {
	kind byte // 'i' ident, 's' string, 'p' punct
	text string
	line int
}

// gvParse tokenizes and parses top-level statements.
func gvParse(src string) ([]gvNode, error) {
	toks, err := gvLex(src)
	if err != nil {
		return nil, err
	}
	pos := 0
	nodes := gvBlock(toks, &pos, 0)
	if pos < len(toks) {
		return nil, fmt.Errorf("unbalanced braces near line %d", toks[pos].line)
	}
	return nodes, nil
}

func gvLex(src string) ([]gvToken, error) {
	var toks []gvToken
	line := 1
	i := 0
	for i < len(src) {
		c := src[i]
		switch {
		case c == '\n':
			line++
			i++
		case c == ' ' || c == '\t' || c == '\r':
			i++
		case c == '/' && i+1 < len(src) && src[i+1] == '/':
			for i < len(src) && src[i] != '\n' {
				i++
			}
		case c == '/' && i+1 < len(src) && src[i+1] == '*':
			i += 2
			for i+1 < len(src) && !(src[i] == '*' && src[i+1] == '/') {
				if src[i] == '\n' {
					line++
				}
				i++
			}
			i += 2
		case c == '\'' || c == '"':
			quote := string(c)
			if strings.HasPrefix(src[i:], quote+quote+quote) {
				quote = quote + quote + quote
			}
			end := i + len(quote)
			var sb strings.Builder
			for {
				if end >= len(src) {
					return nil, fmt.Errorf("unterminated string at line %d", line)
				}
				if src[end] == '\\' && end+1 < len(src) && len(quote) == 1 {
					sb.WriteByte(gvEscape(src[end+1]))
					end += 2
					continue
				}
				if strings.HasPrefix(src[end:], quote) {
					break
				}
				if src[end] == '\n' {
					line++
				}
				sb.WriteByte(src[end])
				end++
			}
			toks = append(toks, gvToken{kind: 's', text: sb.String(), line: line})
			i = end + len(quote)
		case gvIdentByte(c):
			j := i
			for j < len(src) && gvIdentByte(src[j]) {
				j++
			}
			toks = append(toks, gvToken{kind: 'i', text: src[i:j], line: line})
			i = j
		default:
			toks = append(toks, gvToken{kind: 'p', text: string(c), line: line})
			i++
		}
	}
	return toks, nil
}

func gvEscape(c byte) byte {
	switch c {
	case 'n':
		return '\n'
	case 't':
		return '\t'
	default:
		return c
	}
}

func gvIdentByte(c byte) bool {
	return c == '_' || c == '$' || c == '.' ||
		(c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')
}

// gvBlock parses statements until the matching close brace (depth > 0) or
// the end of input. A statement is an identifier plus everything on its
// line (parens tracked), optionally followed by a `{ ... }` block.
func gvBlock(toks []gvToken, pos *int, depth int) []gvNode {
	var nodes []gvNode
	for *pos < len(toks) {
		t := toks[*pos]
		if t.kind == 'p' && t.text == "}" {
			if depth > 0 {
				*pos++
			}
			return nodes
		}
		if t.kind != 'i' && t.kind != 's' {
			*pos++ // stray punctuation between statements
			continue
		}
		node := gvNode{name: t.text}
		startLine := t.line
		*pos++
		parens := 0
		for *pos < len(toks) {
			a := toks[*pos]
			if a.kind == 'p' {
				switch a.text {
				case "(":
					parens++
					*pos++
					continue
				case ")":
					parens--
					*pos++
					continue
				case "{":
					*pos++
					node.block = gvBlock(toks, pos, depth+1)
					goto done
				case "}":
					goto done
				default:
					if parens == 0 && a.line > startLine {
						goto done
					}
					node.args = append(node.args, a.text)
					node.literal = append(node.literal, false)
					*pos++
					continue
				}
			}
			// A fresh identifier on a new line starts the next statement.
			if a.kind == 'i' && parens == 0 && a.line > startLine {
				goto done
			}
			node.args = append(node.args, a.text)
			node.literal = append(node.literal, a.kind == 's')
			*pos++
		}
	done:
		nodes = append(nodes, node)
	}
	return nodes
}
