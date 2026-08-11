# Symfony on Unikraft

A Symfony HTTP service running as a Unikraft unikernel, built with `bsdkrun pack`.

Detected as PHP by its `composer.json`; the dependencies are installed for real
at build time by composer, which `pack` copies in from composer's own image.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "php -- /usr/local/bin/php -S 0.0.0.0:8080 -t /usr/src/public /usr/src/public/index.php"
```

## Try it

```sh
curl http://<vm-ip>:8080/
```

```json
{"message":"Hello from Symfony on Unikraft!","php":"8.2.33","symfony":"components","path":"\/"}
```

## Why `php -S`

A `public/index.php` means a framework front controller, and those expect a web
SAPI: the entry point reads the request from superglobals and never listens on a
socket. Run as a plain CLI script it would answer a request that never came, and
exit. `pack` spots the `public/` document root and serves it with PHP's built-in
server — single-process, which suits a guest that is a single CPU.

Set `BSDKRUN_PHP_SERVER=frankenphp` to use FrankenPHP instead. That path is
**experimental and not yet booted**: FrankenPHP is a Go binary, so on x86_64 it
has to be compiled rather than downloaded — a released one loads at exactly the
address the `fc` kernel occupies.

nginx + php-fpm is not offered. php-fpm is a process manager that `fork()`s its
workers, and Unikraft has no `fork()`.

## Components, not the full skeleton

This uses `symfony/http-foundation` and `symfony/routing` directly. The
full-stack skeleton would add a container, a cache directory and a boot sequence
without changing what is being shown: that composer dependencies reach the guest
and a Symfony request/response cycle runs there.
