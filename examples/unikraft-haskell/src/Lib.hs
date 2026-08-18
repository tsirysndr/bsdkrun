-- The response, factored out of Main so the test suite can assert exactly
-- what the unikernel serves without a listener.
{-# LANGUAGE OverloadedStrings #-}

module Lib
  ( port
  , body
  , response
  ) where

import qualified Data.ByteString.Char8 as B
import Data.Version (showVersion)
import Network.Socket (PortNumber)
import System.Info (compilerVersion)

port :: PortNumber
port = 8080

body :: B.ByteString
body =
  B.concat
    [ "{\"message\": \"Hello from Haskell on Unikraft!\", \"ghc\": \""
    , B.pack (showVersion compilerVersion)
    , "\"}"
    ]

response :: B.ByteString
response =
  B.concat
    [ "HTTP/1.1 200 OK\r\n"
    , "Content-Type: application/json\r\n"
    , "Content-Length: "
    , B.pack (show (B.length body))
    , "\r\n"
    , "Connection: close\r\n\r\n"
    , body
    ]
