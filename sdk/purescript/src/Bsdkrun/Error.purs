module Bsdkrun.Error (ErrorKind(..), kind, commandResult, graphqlCode) where

import Prelude
import Effect.Exception (Error)
import Bsdkrun.Types (CommandResult)
import Data.Maybe (Maybe)
import Data.Nullable (Nullable, toMaybe)

data ErrorKind = BinaryNotFound | CommandFailed | SandboxNotFound | FileTransferFailed | GraphqlError | AuthError | Other

kind :: Error -> ErrorKind
kind err = case errorName err of
  "BinaryNotFoundError" -> BinaryNotFound
  "CommandFailedError" -> CommandFailed
  "SandboxNotFoundError" -> SandboxNotFound
  "FileTransferError" -> FileTransferFailed
  "GraphQLError" -> GraphqlError
  "AuthError" -> AuthError
  _ -> Other

commandResult :: Error -> Maybe CommandResult
commandResult = toMaybe <<< resultImpl

graphqlCode :: Error -> Maybe String
graphqlCode = toMaybe <<< codeImpl

foreign import errorName :: Error -> String
foreign import resultImpl :: Error -> Nullable CommandResult
foreign import codeImpl :: Error -> Nullable String
