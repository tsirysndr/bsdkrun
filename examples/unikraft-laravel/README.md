# Laravel on Unikraft

A Laravel 12 application running as a Unikraft unikernel, built with
`bsdkrun pack`.

Detected as PHP by its `composer.json`. The framework itself — all of
`laravel/framework` and its dependencies — is installed by composer at build
time and ends up in the guest.

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
{"message":"Hello from Laravel on Unikraft!","laravel":"12.65.0","php":"8.2.33"}
```

## What the guest needs that a server does not

| Setting | Why |
| ------- | --- |
| `APP_KEY` | Laravel refuses to boot without one — `MissingAppKeyException`, before any route runs. The committed key is a demo: a unikernel image is not where a real one would live. |
| `LOG_CHANNEL=stderr` | There is no syslog and no filesystem worth persisting to. stderr is the serial console, which is the only place anyone will read it. |
| `SESSION_DRIVER=array`, `CACHE_STORE=array` | The file drivers write into `storage/`, on a ramdisk that does not outlive the guest. |

`storage/` and `bootstrap/cache` still ship with `.gitkeep` files: git does not
track empty directories, and the framework expects them to exist.

## Why `php -S`

Laravel's front controller expects a web SAPI — see
[`../unikraft-symfony/README.md`](../unikraft-symfony/README.md), which explains
the same mechanism and the `BSDKRUN_PHP_SERVER=frankenphp` alternative.
