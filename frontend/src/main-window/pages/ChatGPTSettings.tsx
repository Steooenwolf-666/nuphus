import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useLanguage } from '../../locales'
import { Section } from '../../ui/PageLayout'
import { Button } from '../../ui/Button'
import { getCurrentConfig, openExternal, switchModel } from '../lib/api'

interface Account {
  account_id: string
  provider: string
  label: string
  logged_in: boolean
}
interface Model {
  slug: string
  display_name: string
}

export function ChatGPTSettings({ onModelChanged }: { onModelChanged?: () => void }) {
  const { lang } = useLanguage()
  const zh = lang.startsWith('zh')
  const [accounts, setAccounts] = useState<Account[]>([])
  const [selected, setSelected] = useState('')
  const [models, setModels] = useState<Model[]>([])
  const [busy, setBusy] = useState('accounts')
  const [catalogRevision, setCatalogRevision] = useState(0)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [active, setActive] = useState('')
  const generation = useRef(0)
  const account = accounts.find(a => a.account_id === selected)

  useEffect(() => {
    let mounted = true
    Promise.all([invoke<Account[]>('chatgpt_accounts'), getCurrentConfig().catch(() => null)])
      .then(([items, config]) => {
        if (mounted) {
          setAccounts(items)
          setSelected(
            (items.find(a => a.provider === config?.provider) ?? items[0])?.account_id ?? '',
          )
          if (config) setActive(`${config.provider}:${config.model}`)
          setBusy('')
        }
      })
      .catch(e => {
        if (mounted) {
          setError(String(e))
          setBusy('')
        }
      })
    return () => {
      mounted = false
      generation.current++
      void invoke('chatgpt_cancel_login').catch(() => {})
    }
  }, [])

  async function loadModels(id: string) {
    const current = ++generation.current
    setModels([])
    setBusy('models')
    setError('')
    try {
      const list = await invoke<Model[]>('chatgpt_models', { accountId: id })
      if (generation.current === current) setModels(list)
    } catch (e) {
      if (generation.current === current) setError(String(e))
    } finally {
      if (generation.current === current) setBusy('')
    }
  }

  useEffect(() => {
    if (account?.logged_in) void loadModels(account.account_id)
    else {
      generation.current++
      setModels([])
    }
  }, [selected, account?.logged_in, catalogRevision])

  async function login(id?: string) {
    setBusy('login')
    setError('')
    setNotice('')
    const current = ++generation.current
    try {
      const result = await invoke<Account>('chatgpt_login', { accountId: id ?? null })
      const list = await invoke<Account[]>('chatgpt_accounts')
      if (generation.current !== current) return
      setAccounts(list)
      setSelected(result.account_id)
      // Reauthorization may keep the same selected/logged-in values.
      setCatalogRevision(revision => revision + 1)
    } catch (e) {
      if (generation.current === current) setError(String(e))
    } finally {
      if (generation.current === current) setBusy('')
    }
  }

  async function logout() {
    if (!account) return
    setBusy('logout')
    setError('')
    setNotice('')
    try {
      const revoked = await invoke<boolean>('chatgpt_logout', { accountId: account.account_id })
      setAccounts(await invoke<Account[]>('chatgpt_accounts'))
      setModels([])
      setNotice(
        revoked
          ? zh
            ? '已退出登录。'
            : 'Signed out.'
          : zh
            ? '已在本机退出，但未确认远端撤销。请到 ChatGPT 设置中断开此应用。'
            : 'Signed out locally; remote revocation was not confirmed. Disconnect this app in ChatGPT Settings.',
      )
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy('')
    }
  }

  async function selectModel(model: Model) {
    if (!account) return
    setBusy('switch')
    setError('')
    try {
      await switchModel(model.slug, account.provider, undefined, undefined, 'default')
      setActive(`${account.provider}:${model.slug}`)
      onModelChanged?.()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy('')
    }
  }

  return (
    <Section
      title="ChatGPT"
      description={
        zh
          ? '登录 ChatGPT，授权 Nuphus 使用你的套餐额度。可用模型与用量取决于账号和工作区。'
          : 'Sign in to let Nuphus use your ChatGPT plan. Models and usage depend on your account and workspace.'
      }
    >
      <p>
        {zh
          ? '此连接使用 ChatGPT 套餐用量；API Key 连接另行计费。'
          : 'This connection uses your ChatGPT plan; API key connections are billed separately.'}
      </p>
      {accounts.length > 0 && (
        <label>
          {zh ? 'ChatGPT 账号 / 工作区' : 'ChatGPT account / workspace'}
          <select
            aria-label={zh ? 'ChatGPT 账号' : 'ChatGPT account'}
            value={selected}
            disabled={!!busy}
            onChange={e => {
              setSelected(e.target.value)
              setNotice('')
              setError('')
            }}
          >
            {accounts.map(a => (
              <option key={a.account_id} value={a.account_id}>
                {a.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className="custom-oauth-actions">
        <Button disabled={!!busy} onClick={() => void login(account?.account_id)}>
          Continue with ChatGPT
        </Button>
        {accounts.length > 0 && (
          <Button disabled={!!busy} onClick={() => void login()}>
            {zh ? '添加账号 / 工作区' : 'Add account / workspace'}
          </Button>
        )}
        {account?.logged_in && (
          <>
            <Button disabled={!!busy} onClick={() => void loadModels(account.account_id)}>
              {zh ? '刷新模型' : 'Refresh models'}
            </Button>
            <Button disabled={!!busy} onClick={() => void logout()}>
              {zh ? '退出登录' : 'Sign out'}
            </Button>
          </>
        )}
        {busy === 'login' && (
          <Button onClick={() => void invoke('chatgpt_cancel_login')}>
            {zh ? '取消登录' : 'Cancel sign-in'}
          </Button>
        )}
        <Button
          onClick={() =>
            void openExternal('https://chatgpt.com/#settings/Usage').catch(e => setError(String(e)))
          }
        >
          {zh ? '查看 ChatGPT 用量' : 'ChatGPT usage'}
        </Button>
      </div>
      {busy && (
        <p role="status">
          {busy === 'login'
            ? zh
              ? '请在浏览器完成登录与授权…'
              : 'Complete sign-in and consent in your browser…'
            : zh
              ? '处理中…'
              : 'Working…'}
        </p>
      )}
      {error && (
        <p role="alert" className="custom-oauth-feedback is-err">
          {error}
        </p>
      )}
      {notice && <p role="status">{notice}</p>}
      <div className="custom-oauth-body">
        {models.map(model => (
          <Button key={model.slug} disabled={!!busy} onClick={() => void selectModel(model)}>
            {model.display_name}
            {active === `${account?.provider}:${model.slug}`
              ? zh
                ? ' · 使用中'
                : ' · Active'
              : ''}
          </Button>
        ))}
        {account?.logged_in && !busy && !error && models.length === 0 && (
          <p>
            {zh
              ? '此账号当前没有可用模型。'
              : 'No models are currently available for this account.'}
          </p>
        )}
      </div>
    </Section>
  )
}
