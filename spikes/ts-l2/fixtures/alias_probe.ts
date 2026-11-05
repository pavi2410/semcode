import { z as zz } from "zod/v4";
import * as zm from "./v4/classic/external.js";

export const s = zz.string();
export const r = s.safeParse(1);
export const o = zm.object({ a: zz.string(), b: zz.number() });
export const ключ = "日本語🎉"; export const r2 = o.safeParse(ключ);
