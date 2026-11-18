import { greet } from "./lib/greet.ts";
import { shout } from "./vendor/tiny-pkg.js";

const heading: HTMLElement | null = document.querySelector("h1");
if (heading) {
  heading.title = shout(greet(heading.textContent ?? ""));
}
