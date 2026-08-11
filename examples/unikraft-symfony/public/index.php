<?php
// A Symfony HTTP service, to prove it runs as a Unikraft unikernel.
//
// HttpFoundation and Routing rather than the full framework skeleton: the
// example is about composer dependencies reaching the guest, and the
// full-stack skeleton adds a container, a cache directory and a boot
// sequence without changing what is being demonstrated.

require dirname(__DIR__) . '/vendor/autoload.php';

use Symfony\Component\HttpFoundation\JsonResponse;
use Symfony\Component\HttpFoundation\Request;
use Symfony\Component\Routing\Matcher\UrlMatcher;
use Symfony\Component\Routing\RequestContext;
use Symfony\Component\Routing\Route;
use Symfony\Component\Routing\RouteCollection;

$routes = new RouteCollection();
$routes->add('home', new Route('/', ['_controller' => 'home']));

$request = Request::createFromGlobals();

$context = new RequestContext();
$context->fromRequest($request);
$matcher = new UrlMatcher($routes, $context);

try {
    $matcher->match($request->getPathInfo());
    $response = new JsonResponse([
        'message' => 'Hello from Symfony on Unikraft!',
        'php' => PHP_VERSION,
        'symfony' => 'components',
        'path' => $request->getPathInfo(),
    ]);
} catch (\Symfony\Component\Routing\Exception\ResourceNotFoundException) {
    $response = new JsonResponse(['error' => 'not found'], 404);
}

$response->prepare($request)->send();
