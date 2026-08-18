<?php

namespace Tests;

use PHPUnit\Framework\TestCase;

// The front controller does everything at include time and send()s, so the
// honest test runs it exactly as deployed: php's built-in server serving
// public/, and the assertions read the same JSON the unikernel e2e reads.
final class SmokeTest extends TestCase
{
    public function testRootGreets(): void
    {
        $proc = proc_open(
            [PHP_BINARY, '-S', '127.0.0.1:8091', '-t', 'public'],
            [1 => ['file', '/dev/null', 'w'], 2 => ['file', '/dev/null', 'w']],
            $pipes,
            dirname(__DIR__),
        );
        $this->assertIsResource($proc);

        try {
            $body = false;
            for ($i = 0; $i < 50 && $body === false; $i++) {
                usleep(100_000);
                $body = @file_get_contents('http://127.0.0.1:8091/');
            }
            $this->assertNotFalse($body, 'server never came up');
            $json = json_decode($body, true);
            $this->assertSame('Hello from Symfony on Unikraft!', $json['message']);
        } finally {
            proc_terminate($proc);
        }
    }
}
