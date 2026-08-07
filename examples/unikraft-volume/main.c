/*
 * Proves a persistent volume works: reads a counter from /data/counter,
 * increments it, writes it back. Boot it twice against the same host
 * directory and the count must go up — which it can only do if the write
 * reached the host and survived the VM.
 */

#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define PATH "/data/counter"

int main(int argc, char *argv[]) {
	char buf[32];
	long n = 0;
	int fd;

	printf("volume test: reading %s\n", PATH);

	fd = open(PATH, O_RDONLY);
	if (fd < 0) {
		printf("  (no counter yet — first boot)\n");
	} else {
		ssize_t r = read(fd, buf, sizeof(buf) - 1);
		close(fd);
		if (r > 0) {
			buf[r] = '\0';
			n = 0;
			for (ssize_t i = 0; i < r && buf[i] >= '0' && buf[i] <= '9'; i++)
				n = n * 10 + (buf[i] - '0');
			printf("  read counter = %ld\n", n);
		}
	}

	n++;

	fd = open(PATH, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if (fd < 0) {
		printf("VOLUME FAIL: cannot open %s for writing\n", PATH);
		return 1;
	}
	int len = snprintf(buf, sizeof(buf), "%ld", n);
	ssize_t w = write(fd, buf, len);
	close(fd);

	if (w != len) {
		printf("VOLUME FAIL: short write (%ld of %d)\n", (long)w, len);
		return 1;
	}

	printf("VOLUME OK: boot count is now %ld\n", n);
	return 0;
}
