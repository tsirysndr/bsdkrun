package bsdkrun

import (
	"fmt"
	"os"
	"strings"
)

// FileTransferError reports a filesystem operation the guest refused. Path
// names the file that could not be transferred.
type FileTransferError struct {
	Path    string
	Message string
}

func (e *FileTransferError) Error() string { return e.Message }

// FileSystem reads and writes files inside a running microVM. Reach it through
// Sandbox.FS.
//
// Every call goes through the guest's exec agent, so the sandbox has to be
// running — there is no offline write.
//
//	sbx.FS().WriteFile("/app/main.py", []byte("print('hi')"))
//	text, _ := sbx.FS().ReadTextFile("/app/out.json")
//	sbx.FS().Upload("./src", "/app/src")
//	sbx.FS().Download("/app/dist", "./dist", true)
type FileSystem struct {
	id string
}

// FS returns a handle to the guest's filesystem.
func (s *Sandbox) FS() *FileSystem { return &FileSystem{id: s.ID} }

// WriteFile writes data to path in the guest, creating parent directories as
// needed.
func (f *FileSystem) WriteFile(path string, data []byte) error {
	res, err := Run([]string{"cp", "-", f.id + ":" + path}, &RunOpts{Stdin: data})
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		return &FileTransferError{Path: path, Message: transferMessage(res.Stderr, path)}
	}
	return nil
}

// WriteTextFile writes text to path in the guest.
func (f *FileSystem) WriteTextFile(path, text string) error {
	return f.WriteFile(path, []byte(text))
}

// ReadFile reads path from the guest as bytes.
func (f *FileSystem) ReadFile(path string) ([]byte, error) {
	res, err := Run([]string{"cp", f.id + ":" + path, "-"}, nil)
	if err != nil {
		return nil, err
	}
	if res.ExitCode != 0 {
		return nil, &FileTransferError{Path: path, Message: transferMessage(res.Stderr, path)}
	}
	// Run buffers stdout into a Go string, which is an arbitrary byte sequence
	// rather than validated UTF-8, so this conversion is lossless for binaries.
	return []byte(res.Stdout), nil
}

// ReadTextFile reads path from the guest as a string.
func (f *FileSystem) ReadTextFile(path string) (string, error) {
	data, err := f.ReadFile(path)
	return string(data), err
}

// Upload copies a host file or directory into the guest. A directory's
// *contents* land in remotePath, so Upload("./src", "/app/src") leaves the
// guest's /app/src holding what ./src holds. Whether it recurses is decided by
// looking at localPath, so callers do not have to say which kind of thing it is.
func (f *FileSystem) Upload(localPath, remotePath string) error {
	info, err := os.Stat(localPath)
	if err != nil {
		return &FileTransferError{
			Path:    localPath,
			Message: fmt.Sprintf("cannot upload %s: %v", localPath, err),
		}
	}
	args := []string{"cp"}
	if info.IsDir() {
		args = append(args, "-r")
	}
	args = append(args, localPath, f.id+":"+remotePath)
	res, err := Run(args, nil)
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		return &FileTransferError{Path: localPath, Message: transferMessage(res.Stderr, localPath)}
	}
	return nil
}

// Download copies a file or directory out of the guest onto the host. Pass
// recursive for a directory; unlike Upload it cannot be detected here, because
// the path lives in the guest and answering would cost an extra round trip.
func (f *FileSystem) Download(remotePath, localPath string, recursive bool) error {
	args := []string{"cp"}
	if recursive {
		args = append(args, "-r")
	}
	args = append(args, f.id+":"+remotePath, localPath)
	res, err := Run(args, nil)
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		return &FileTransferError{Path: remotePath, Message: transferMessage(res.Stderr, remotePath)}
	}
	return nil
}

// transferMessage reuses the CLI's diagnostic, minus its "Error: " prefix.
func transferMessage(stderr, path string) string {
	text := strings.TrimPrefix(strings.TrimSpace(stderr), "Error: ")
	if text == "" {
		return "file transfer failed for " + path
	}
	return text
}
