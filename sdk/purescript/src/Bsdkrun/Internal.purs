module Bsdkrun.Internal where

import Prelude
import Data.Either (Either(..))
import Effect (Effect)
import Effect.Aff (Aff, Canceler(..), makeAff)
import Effect.Class (liftEffect)
import Effect.Exception (Error)

-- | Cold asynchronous operation. Cancellation suppresses callbacks and aborts
-- | subprocesses where supported by the underlying runtime.
type Async a = (Error -> Effect Unit) -> (a -> Effect Unit) -> Effect (Effect Unit)

toAff :: forall a. Async a -> Aff a
toAff task = makeAff \done -> do
  cancel <- task (done <<< Left) (done <<< Right)
  pure $ Canceler \_ -> liftEffect cancel

foreign import data Promise :: Type -> Type
foreign import data AbortSignal :: Type
foreign import async :: forall a. (AbortSignal -> Promise a) -> Async a
