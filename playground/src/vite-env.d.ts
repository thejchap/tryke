/// <reference types="vite/client" />

declare module "*.py?raw" {
  const content: string;
  export default content;
}
