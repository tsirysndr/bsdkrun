-- Asserts the exact bytes the unikernel e2e asserts: the greeting in the
-- body, and a Content-Length that tells the truth.
{-# LANGUAGE OverloadedStrings #-}

module Main (main) where

import Control.Monad (unless)
import qualified Data.ByteString.Char8 as B
import Lib (body, response)
import System.Exit (exitFailure)

main :: IO ()
main = do
  unless ("Hello from Haskell on Unikraft!" `B.isInfixOf` body) $ do
    putStrLn "greeting missing from body"
    exitFailure
  let declared = B.pack ("Content-Length: " ++ show (B.length body))
  unless (declared `B.isInfixOf` response) $ do
    putStrLn "Content-Length does not match the body"
    exitFailure
  putStrLn "ok"
