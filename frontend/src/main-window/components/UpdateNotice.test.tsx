import { StrictMode, useState } from 'react'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { useAppUpdate } from '../hooks/useAppUpdate'
import { UpdatePage } from '../pages/UpdatePage'
import { UpdateNotice } from './UpdateNotice'

vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn() }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '1.0.0') }))
vi.mock('../lib/api', () => ({ getChangelog: vi.fn(async () => '') }))

/** 与 App 相同的生命周期：控制器常驻，手动页按弹窗开关挂载。 */
function Harness({ ready = true }: { ready?: boolean }) {
  const updater = useAppUpdate(ready)
  const [open, setOpen] = useState(false)
  return (
    <>
      <UpdateNotice updater={updater} onOpen={() => setOpen(true)} />
      <button onClick={() => setOpen(true)}>打开更新页</button>
      {open && (
        <div role="dialog">
          <button onClick={() => setOpen(false)}>关闭更新页</button>
          <UpdatePage updater={updater} />
        </div>
      )}
    </>
  )
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function release(version = '1.1.0') {
  const update = {
    version,
    body: '新版本说明',
    download: vi.fn(async (_onEvent?: (event: DownloadEvent) => void) => {}),
    install: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
  }
  return { ...update, resource: update as unknown as Update }
}

async function advance(ms = 5_000) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.resetAllMocks()
  vi.stubGlobal('__TAURI_INTERNALS__', {})
  vi.mocked(check).mockResolvedValue(null)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('启动更新提示与手动更新共用流程', () => {
  it('主窗口就绪后只检查一次；StrictMode 不重复检查，未点击不下载', async () => {
    const update = release()
    vi.mocked(check).mockResolvedValue(update.resource)
    const view = render(
      <StrictMode>
        <Harness ready={false} />
      </StrictMode>,
    )
    await advance(10_000)
    expect(check).not.toHaveBeenCalled()
    view.rerender(
      <StrictMode>
        <Harness ready />
      </StrictMode>,
    )
    await advance(4_999)
    expect(check).not.toHaveBeenCalled()
    await advance(1)
    expect(check).toHaveBeenCalledExactlyOnceWith({ timeout: 10_000 })
    expect(screen.getByRole('status')).toHaveTextContent('发现新版本 v1.1.0')
    expect(screen.getByRole('button', { name: '下载并安装' })).toBeEnabled()
    await advance(60_000)
    expect(check).toHaveBeenCalledTimes(1)
    expect(update.download).not.toHaveBeenCalled()
    expect(update.install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()
  })

  it('普通浏览器预览不会自动调用桌面 updater', async () => {
    vi.unstubAllGlobals()
    render(<Harness />)
    await advance()
    expect(check).not.toHaveBeenCalled()
  })

  it('没有新版时不显示提示，也不自动打开弹窗', async () => {
    render(<Harness />)
    await advance()
    expect(screen.queryByRole('status')).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('后台检查失败保持安静，手动打开可看到原因并重试', async () => {
    vi.mocked(check).mockRejectedValueOnce(new Error('network timeout'))
    render(<Harness />)
    await advance()
    expect(screen.queryByRole('status')).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
    fireEvent.click(screen.getByText('打开更新页'))
    expect(screen.getByText(/network timeout/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '重试' }))
    await advance(0)
    expect(screen.getByText('当前已是最新版本')).toBeInTheDocument()
  })

  it('手动检查先开始时取消自动重复检查，在途检查不能叠加', async () => {
    const pending = deferred<Update | null>()
    vi.mocked(check).mockReturnValue(pending.promise)
    render(<Harness />)
    fireEvent.click(screen.getByText('打开更新页'))
    fireEvent.click(screen.getByRole('button', { name: '检查更新' }))
    await advance()
    expect(check).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('button', { name: '检查更新' })).toBeDisabled()
    await act(async () => pending.resolve(null))
  })

  it('自动检查进行中打开手动页，会显示相同的检查状态和结果', async () => {
    const pending = deferred<Update | null>()
    const update = release()
    vi.mocked(check).mockReturnValue(pending.promise)
    render(<Harness />)
    await advance()
    fireEvent.click(screen.getByText('打开更新页'))
    expect(screen.getByText('正在检查官方版本…')).toBeInTheDocument()
    await act(async () => pending.resolve(update.resource))
    expect(screen.getByRole('dialog')).toHaveTextContent('发现新版本 v1.1.0')
    expect(check).toHaveBeenCalledTimes(1)
  })

  it('稍后仅收起当前版本提示，手动入口保留结果；再次检测到不同版本会提醒', async () => {
    const first = release()
    const second = release('1.2.0')
    vi.mocked(check).mockResolvedValueOnce(first.resource).mockResolvedValueOnce(second.resource)
    render(<Harness />)
    await advance()
    fireEvent.click(screen.getByRole('button', { name: '稍后' }))
    expect(screen.queryByRole('status')).toBeNull()
    fireEvent.click(screen.getByText('打开更新页'))
    expect(screen.getByText('发现新版本 v1.1.0')).toBeInTheDocument()
    expect(check).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: '检查更新' }))
    await advance(0)
    expect(screen.getByRole('status')).toHaveTextContent('发现新版本 v1.2.0')
    expect(first.close).toHaveBeenCalledTimes(1)
    expect(first.download).not.toHaveBeenCalled()
  })

  it('点击后只下载一次，关闭和重开更新页保持进度，下载完成才安装重启', async () => {
    const update = release()
    const download = deferred<void>()
    let progress!: (event: DownloadEvent) => void
    update.download.mockImplementation(onEvent => {
      progress = onEvent!
      return download.promise
    })
    vi.mocked(check).mockResolvedValue(update.resource)
    render(<Harness />)
    await advance()
    fireEvent.click(screen.getByRole('button', { name: '下载并安装' }))
    expect(update.download).toHaveBeenCalledExactlyOnceWith(expect.any(Function), {
      timeout: 600_000,
    })
    expect(update.install).not.toHaveBeenCalled()
    act(() => {
      progress({ event: 'Started', data: { contentLength: 100 } })
      progress({ event: 'Progress', data: { chunkLength: 35 } })
    })
    fireEvent.click(screen.getByText('关闭更新页'))
    expect(screen.getByRole('status')).toHaveTextContent('35%')
    fireEvent.click(screen.getByRole('button', { name: '查看详情' }))
    expect(screen.getByRole('progressbar')).toHaveAttribute('value', '35')
    expect(screen.queryByRole('button', { name: '下载并安装' })).toBeNull()
    expect(screen.queryByRole('button', { name: '检查更新' })).toBeNull()
    expect(update.download).toHaveBeenCalledTimes(1)
    expect(check).toHaveBeenCalledTimes(1)
    await act(async () => download.resolve())
    expect(update.install).toHaveBeenCalledExactlyOnceWith({ restartAfterInstall: true })
    expect(relaunch).toHaveBeenCalledTimes(1)
  })

  it('下载停滞提示在进度恢复后消失', async () => {
    const update = release()
    const download = deferred<void>()
    let progress!: (event: DownloadEvent) => void
    update.download.mockImplementation(onEvent => {
      progress = onEvent!
      return download.promise
    })
    vi.mocked(check).mockResolvedValue(update.resource)
    render(<Harness />)
    await advance()
    fireEvent.click(screen.getByRole('button', { name: '下载并安装' }))
    await advance(10_000)
    expect(screen.getByText(/下载进展缓慢或停滞/)).toBeInTheDocument()
    act(() => progress({ event: 'Progress', data: { chunkLength: 20 } }))
    expect(screen.queryByText(/下载进展缓慢或停滞/)).toBeNull()
    await act(async () => download.resolve())
  })

  it('签名校验失败时不安装和重启，保留错误与手动重试入口', async () => {
    const update = release()
    update.download.mockRejectedValue(new Error('minisign: signature verification failed'))
    vi.mocked(check).mockResolvedValue(update.resource)
    render(<Harness />)
    await advance()
    fireEvent.click(screen.getByRole('button', { name: '下载并安装' }))
    await advance(0)
    expect(screen.getByText(/安装包签名校验未通过/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '重试' })).toBeEnabled()
    expect(update.install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()
  })

  it('主窗口卸载时取消未开始的启动检查', async () => {
    const view = render(<Harness />)
    view.unmount()
    await advance()
    expect(check).not.toHaveBeenCalled()
  })

  it('主窗口卸载后检查才返回时，释放更新资源', async () => {
    const pending = deferred<Update | null>()
    const update = release()
    vi.mocked(check).mockReturnValue(pending.promise)
    const view = render(<Harness />)
    await advance()
    view.unmount()
    await act(async () => pending.resolve(update.resource))
    expect(update.close).toHaveBeenCalledTimes(1)
  })

  it('主窗口在下载时卸载，结束后释放资源而不继续安装', async () => {
    const update = release()
    const download = deferred<void>()
    update.download.mockReturnValue(download.promise)
    vi.mocked(check).mockResolvedValue(update.resource)
    const view = render(<Harness />)
    await advance()
    fireEvent.click(screen.getByRole('button', { name: '下载并安装' }))
    view.unmount()
    expect(update.close).not.toHaveBeenCalled()
    await act(async () => download.resolve())
    expect(update.close).toHaveBeenCalledTimes(1)
    expect(update.install).not.toHaveBeenCalled()
  })
})
