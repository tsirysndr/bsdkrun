import { expect, test } from "bun:test";
import { slugify } from "./slug";

test("spaces and case", () => {
  expect(slugify("Hello World")).toBe("hello-world");
});

test("accents fold to ascii", () => {
  expect(slugify("Café con Leche")).toBe("cafe-con-leche");
});

test("punctuation collapses", () => {
  expect(slugify("  a -- b!! c  ")).toBe("a-b-c");
});
