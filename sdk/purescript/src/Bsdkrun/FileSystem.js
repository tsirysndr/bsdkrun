import { async } from "../Bsdkrun.Internal/foreign.js";

export const writeTextImpl = path => text => box => async(signal => box.fs.writeFile(path, text, {signal}));
export const writeFileImpl = path => bytes => box => async(signal => box.fs.writeFile(path, bytes, {signal}));
export const readTextImpl = path => box => async(signal => box.fs.readTextFile(path, {signal}));
export const readFileImpl = path => box => async(signal => box.fs.readFile(path, {signal}));
export const uploadImpl = local => remote => box => async(signal => box.fs.upload(local, remote, {signal}));
export const downloadImpl = remote => local => recursive => box => async(signal => box.fs.download(remote, local, {recursive, signal}));
