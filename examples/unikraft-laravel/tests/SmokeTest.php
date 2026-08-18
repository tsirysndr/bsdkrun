<?php

namespace Tests;

use Illuminate\Contracts\Http\Kernel;
use Illuminate\Http\Request;
use PHPUnit\Framework\TestCase;

// Boot the same application the unikernel boots and dispatch the same
// request the e2e workflow sends — the whole stack, no listener. Through
// the kernel contract, deliberately: handleRequest() is the front
// controller's entrypoint and *sends* the response (returning void);
// the kernel returns it.
final class SmokeTest extends TestCase
{
    public function testRootGreets(): void
    {
        $app = require __DIR__ . '/../bootstrap/app.php';
        $response = $app->make(Kernel::class)->handle(Request::create('/'));

        $this->assertSame(200, $response->getStatusCode());
        $this->assertStringContainsString(
            'Hello from Laravel on Unikraft!',
            $response->getContent(),
        );
    }
}
