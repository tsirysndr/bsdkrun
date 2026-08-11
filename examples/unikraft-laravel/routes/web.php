<?php

use Illuminate\Support\Facades\Route;

Route::get('/', fn () => response()->json([
    'message' => 'Hello from Laravel on Unikraft!',
    'laravel' => app()->version(),
    'php' => PHP_VERSION,
]));
