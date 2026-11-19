import { async } from "../Bsdkrun.Internal/foreign.js";
import * as SDK from "@bsdkrun/sdk";
export const connectImpl = url => token => async(signal => (() => { if (!url.trim() || !token.trim()) throw new Error("URL and token are required"); return new SDK.Client({url, token}); })());
export const fromEnvImpl = async(signal => SDK.Client.fromEnv());
export const requestImpl = query => variables => client => async(signal => client.request(query, variables));
export const listImpl = all => client => async(signal => client.list(all));
const select = (opts, fields) => Object.fromEntries(fields.filter(key => opts[key] !== undefined).map(key => [key, opts[key]]));
const netOptions = opts => ({...select(opts.net ?? {}, ["ports", "mac", "network"]), noNet: opts.net?.disabled ?? false, ...(opts.name === undefined ? {} : {name: opts.name})});
const linuxOptions = opts => ({...select(opts, ["image", "cpus", "mem", "volume", "mounts", "attachDisk", "entrypoint", "initramfs", "kernel", "kernelVersion", "console", "command"]), net: netOptions(opts), env: Object.keys(opts.env ?? {}).sort().map(key => `${key}=${opts.env[key]}`)});
export const runLinuxImpl = opts => client => async(() => {
  if (opts.os !== "linux") throw new Error("runLinux requires Linux options");
  return client.runLinux(linuxOptions(opts));
});
export const execImpl = id => command => env => client => async(signal => client.exec(id, command, {env}));
export const stopImpl = id => client => async(signal => client.stop(id));
export const startImpl = id => client => async(signal => client.start(id));
export const removeImpl = ids => force => client => async(signal => client.remove(ids, force));
export const logsImpl = id => boot => client => async(signal => client.logs(id, boot));

export const getImpl = id => client => async(signal => client.get(id));
export const updateImpl = id => cpus => mem => client => async(signal => client.update(id, {cpus, mem}));
export const commitImpl = id => name => description => client => async(signal => client.commit(id, name, description));
const bsdOptions = opts => ({...select(opts, ["version", "cpus", "mem", "volume", "persist", "force", "firmware", "attachDisk"]), os: opts.os.toUpperCase(), net: netOptions(opts)});
export const runBsdImpl = opts => client => async(() => {
  if (!["freebsd", "netbsd"].includes(opts.os)) throw new Error("runBsd requires BSD options");
  return client.runBsd(bsdOptions(opts));
});
export const runImpl = opts => client => async(() => {
  if (opts.os === "linux") return client.runLinux(linuxOptions(opts));
  if (["freebsd", "netbsd"].includes(opts.os)) return client.runBsd(bsdOptions(opts));
  const fields = {
    nanos: ["image", "kernel", "cmdline", "persist"],
    unikraft: ["path", "cmdline", "initramfs", "mounts"],
    solo5: ["path", "block", "args"],
    osv: ["image", "cmdline", "disk", "gic", "persist"],
  }[opts.os];
  if (!fields) throw new Error(`The remote daemon cannot boot ${opts.os} guests`);
  const method = {nanos:"runNanos", unikraft:"runUnikraft", solo5:"runSolo5", osv:"runOsv"}[opts.os];
  return client[method]({...select(opts, [...fields, "cpus", "mem"]), net: netOptions(opts)});
});
