export default defineNuxtConfig({
  compatibilityDate: '2026-08-27',
  css: ['~/assets/main.css'],
  devtools: { enabled: false },
  runtimeConfig: {
    graphDatabase: '',
  },
  typescript: {
    strict: true,
    typeCheck: true,
  },
})
