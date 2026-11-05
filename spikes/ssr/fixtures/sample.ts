import { a, b as bee, type C } from './mod';
import {
  x, // first
  y,
} from "pkg";
import def from 'def';

export function f(p: number, q: string) {
  if (p == 1 && q === "a") return p != 2;
  const r = (p + 1) == (q.length /* c */ * 2);
  return r == null;
}
