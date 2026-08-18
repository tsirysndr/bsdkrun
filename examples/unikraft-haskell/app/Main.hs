-- A Haskell HTTP service, to prove GHC output runs as a Unikraft unikernel.
--
-- Raw sockets rather than warp: this example is about the toolchain
-- reaching the guest, and a web server would bring a hundred transitive
-- dependencies with it. The response itself lives in Lib, where the test
-- suite can reach it.
{-# LANGUAGE OverloadedStrings #-}

module Main (main) where

import Control.Monad (forever)
import qualified Data.ByteString.Char8 as B
import Lib (port, response)
import Network.Socket
import Network.Socket.ByteString (recv, sendAll)
import System.IO (hFlush, stdout)

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
