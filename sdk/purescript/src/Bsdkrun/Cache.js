import { async } from "../Bsdkrun.Internal/foreign.js";
import * as SDK from "@bsdkrun/sdk";
export const saveImpl = path => opts => box => async(signal => box.cache.save(path, opts));
export const restoreImpl = opts => box => async(signal => box.cache.restore({...opts, path: opts.path ?? undefined}).then(r => ({...r, key: r.key ?? null, path: r.path ?? null, size: r.size ?? null, compression: r.compression ?? null, created: r.created ?? null})));
export const listImpl = async(signal => SDK.listCaches());
export const removeImpl = keys => all => async(signal => SDK.removeCache(keys, {all}));
