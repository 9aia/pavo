import { defineConfig } from "vitepress";
import llmstxt from "vitepress-plugin-llms";

export default defineConfig({
  vite: {
    plugins: [llmstxt()],
  },
  title: "pavo",
  description:
    "visualization host for the agent factory — tamias drivers emit declarative ViewNode IR; pavo renders it on CLI, TUI, and web",
  base: "/pavo/",
  srcExclude: ["README.md"],
  ignoreDeadLinks: [/\/api\//],
  themeConfig: {
    nav: [
      { text: "Getting Started", link: "/getting-started" },
      { text: "Explanation", link: "/explanation/architecture" },
      { text: "Reference", link: "/reference/cli" },
      {
        text: "LLMs",
        items: [
          { text: "llms.txt", link: "https://9aia.com/pavo/llms.txt", target: "_blank" },
          { text: "llms-full.txt", link: "https://9aia.com/pavo/llms-full.txt", target: "_blank" },
        ],
      },
    ],
    sidebar: [
      { text: "Getting Started", items: [{ text: "Install pavo", link: "/getting-started" }] },
      {
        text: "Tutorials",
        items: [
          { text: "Your first view", link: "/tutorials/first-view" },
          { text: "Deep links", link: "/tutorials/deep-links" },
        ],
      },
      {
        text: "How-to",
        items: [
          { text: "Install a driver", link: "/how-to/install-a-driver" },
          { text: "Write a driver", link: "/how-to/write-a-driver" },
          { text: "Write a renderer plugin", link: "/how-to/write-a-renderer-plugin" },
          { text: "Serve the web surface", link: "/how-to/serve-web" },
          { text: "Use the config API", link: "/how-to/use-config-api" },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "CLI", link: "/reference/cli" },
          { text: "ViewNode IR", link: "/reference/view-nodes" },
          { text: "Wire ops", link: "/reference/wire-ops" },
          { text: "Driver manifest", link: "/reference/driver-manifest" },
          { text: "Config", link: "/reference/config" },
          { text: "Crate API", link: "/reference/crate-api" },
        ],
      },
      {
        text: "Explanation",
        items: [
          { text: "Architecture", link: "/explanation/architecture" },
          { text: "Why declarative views", link: "/explanation/why-declarative" },
          { text: "Renderer plugins", link: "/explanation/renderer-plugins" },
          { text: "Boundaries", link: "/explanation/boundaries" },
        ],
      },
    ],
    socialLinks: [{ icon: "github", link: "https://github.com/9aia/pavo" }],
  },
});
