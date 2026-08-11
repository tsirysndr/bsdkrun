-- A Haskell HTTP service, to prove GHC output runs as a Unikraft unikernel.
--
-- Raw sockets rather than warp: this example is about the toolchain
-- reaching the guest, and a web server would bring a hundred transitive
-- dependencies with it.
{-# LANGUAGE OverloadedStrings #-}

module Main (main) where

import Control.Monad (forever)
import Data.Version (showVersion)
import qualified Data.ByteString.Char8 as B
import Network.Socket
import Network.Socket.ByteString (recv, sendAll)
import System.IO (hFlush, stdout)
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

main :: IO ()
main = do
  sock <- socket AF_INET Stream defaultProtocol
  bind sock (SockAddrInet port 0)
  listen sock 16

  B.putStrLn (B.concat ["Haskell listening on :", B.pack (show port)])
  hFlush stdout

  forever $ do
    (conn, _) <- accept sock
    _ <- recv conn 1024
    sendAll conn response
    close conn
