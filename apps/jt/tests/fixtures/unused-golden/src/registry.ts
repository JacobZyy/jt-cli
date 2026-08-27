function registeredHandler(): void {}

const handlers = { registeredHandler }
const action = 'registeredHandler' as keyof typeof handlers
handlers[action]()
