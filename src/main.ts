import { mountController } from "./ui/controller";
import { mountBrowser } from "./ui/browser";
import "./styles.css";

const root = document.querySelector<HTMLDivElement>("#app")!;
if (new URLSearchParams(window.location.search).get("view") === "browser") {
  mountBrowser(root);
} else {
  mountController(root);
}
