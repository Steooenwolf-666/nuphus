import { useCallback, useEffect, useRef, useState } from 'react'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

/** 等主窗口就绪后再检查，避免与启动初始化争用网络；每次启动只自动检查一次。 */
const STARTUP_DELAY_MS = 5_000
/** 每个端点的超时会触发 updater 的下一镜像降级，不能依赖其默认无限等待。 */
const CHECK_TIMEOUT_MS = 10_000
/** 下载允许慢速链路，但整体等待有上限；连续十秒无进度时显示停滞提示。 */
const DOWNLOAD_TIMEOUT_MS = 600_000
const STALL_MS = 10_000

type Status = 'idle' | 'checking' | 'available' | 'latest' | 'downloading' | 'error'
interface UpdateState {
  status: Status
  update: Update | null
  progress: number
  stalled: boolean
  error: unknown
}

/** 释放 Rust 侧的更新资源；窗口退出时 IPC 可能已经不可用。 */
function closeUpdate(update: Update | null) {
  if (update) void update.close().catch(() => {})
}

/**
 * 主窗口持有更新状态，启动检查、提示条和手动更新页共用同一份结果。
 * 关闭更新弹窗不会丢失下载进度，也不会允许第二次下载并发启动。
 * 自动检查只读取版本信息；只有 install 的用户点击入口可以下载和安装。
 */
export function useAppUpdate(ready: boolean) {
  const [state, setState] = useState<UpdateState>({
    status: 'idle',
    update: null,
    progress: 0,
    stalled: false,
    error: null,
  })
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null)
  const alive = useRef(false)
  const busy = useRef(false)
  const checked = useRef(false)
  const current = useRef<Update | null>(null)
  const stallTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const publish = useCallback((patch: Partial<UpdateState>) => {
    if (alive.current) setState(previous => ({ ...previous, ...patch }))
  }, [])

  const clearStallTimer = useCallback(() => {
    if (stallTimer.current !== null) clearTimeout(stallTimer.current)
    stallTimer.current = null
  }, [])

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      clearStallTimer()
      /** 在途请求结束后自行释放，避免下载仍在使用资源时提前 close。 */
      if (!busy.current) {
        closeUpdate(current.current)
        current.current = null
      }
    }
  }, [clearStallTimer])

  const checkVersion = useCallback(async () => {
    if (busy.current || !alive.current) return
    checked.current = true
    busy.current = true
    publish({ status: 'checking', error: null })
    try {
      const found = await check({ timeout: CHECK_TIMEOUT_MS })
      if (!alive.current) {
        closeUpdate(found)
        return
      }
      closeUpdate(current.current)
      current.current = found
      publish({ update: found, status: found ? 'available' : 'latest' })
    } catch (error) {
      /** 后台失败不会弹窗；用户打开更新页时仍能查看原因并手动重试。 */
      publish({ status: 'error', error })
    } finally {
      busy.current = false
      if (!alive.current) {
        closeUpdate(current.current)
        current.current = null
      }
    }
  }, [publish])

  useEffect(() => {
    if (!ready || !('__TAURI_INTERNALS__' in window) || checked.current) return
    const timer = setTimeout(() => {
      if (!checked.current) void checkVersion()
    }, STARTUP_DELAY_MS)
    return () => clearTimeout(timer)
  }, [ready, checkVersion])

  const install = useCallback(async () => {
    const update = current.current
    if (!update || busy.current || !alive.current) return
    busy.current = true
    publish({ status: 'downloading', progress: 0, stalled: false, error: null })
    const bump = () => {
      clearStallTimer()
      if (!alive.current) return
      publish({ stalled: false })
      stallTimer.current = setTimeout(() => publish({ stalled: true }), STALL_MS)
    }
    bump()
    try {
      let contentLength: number | undefined
      let downloaded = 0
      await update.download(
        event => {
          bump()
          if (event.event === 'Started') contentLength = event.data.contentLength
          if (event.event === 'Progress') {
            downloaded += event.data.chunkLength
            publish({
              progress: contentLength ? Math.min(100, (downloaded / contentLength) * 100) : 0,
            })
          }
          if (event.event === 'Finished') publish({ progress: 100 })
        },
        { timeout: DOWNLOAD_TIMEOUT_MS },
      )
      clearStallTimer()
      publish({ stalled: false })
      if (!alive.current) return
      await update.install({ restartAfterInstall: true })
      await relaunch()
    } catch (error) {
      publish({ status: 'error', error })
    } finally {
      clearStallTimer()
      publish({ stalled: false })
      busy.current = false
      if (!alive.current) {
        closeUpdate(current.current)
        current.current = null
      }
    }
  }, [publish, clearStallTimer])

  return {
    ...state,
    noticeDismissed: state.update?.version === dismissedVersion,
    dismissNotice: () => setDismissedVersion(state.update?.version ?? null),
    checkVersion,
    install,
  }
}

export type AppUpdate = ReturnType<typeof useAppUpdate>
