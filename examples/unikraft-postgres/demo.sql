-- Fed to the guest on stdin:
--
--   bsdkrun unikraft . --mem 2048 \
--     --cmdline "elfloader -- /usr/local/bin/postgres --single -D /var/lib/postgresql/data postgres" \
--     < demo.sql
--
-- One statement per line, deliberately. The stand-alone backend has no parser
-- loop that waits for a semicolon: it reads a line, runs it, and prints the
-- result in its own tuple-by-tuple debug format. A statement split across two
-- lines is two failed statements.
CREATE TABLE guests (id serial primary key, kernel text, arch text);
INSERT INTO guests (kernel, arch) VALUES ('unikraft', 'arm64'), ('unikraft', 'x86_64');
SELECT count(*) AS rows FROM guests;
SELECT version();
SELECT kernel || '/' || arch AS target FROM guests ORDER BY id;
