import type { AppUpdate } from '../hooks/useAppUpdate'
import { useLanguage } from '../../locales'
import { Button } from '../../ui/Button'
import { IconRefresh } from '../../ui/Icons'
import '../../styles/update.css'

/** 顶部常驻提示不抢焦点、不遮挡对话，下载安装必须由用户明确点击。 */
export function UpdateNotice({ updater, onOpen }: { updater: AppUpdate; onOpen: () => void }) {
  const { t } = useLanguage()
  const { status, update, progress, noticeDismissed } = updater
  const downloading = status === 'downloading'
  if (
    !update ||
    (!downloading && noticeDismissed) ||
    !['available', 'downloading', 'error'].includes(status)
  )
    return null

  return (
    <aside className="update-notice" aria-label={t('app.update')}>
      <span className="update-notice-icon" aria-hidden="true">
        <IconRefresh size={18} />
      </span>
      <div className="update-notice-copy" role="status" aria-live="polite">
        <strong>{t('update.available', update.version)}</strong>
        <span>
          {downloading
            ? t('update.downloading', progress ? `${progress.toFixed(0)}%` : '…')
            : status === 'error'
              ? t('update.noticeFailed')
              : t('update.noticeHint')}
        </span>
      </div>
      <div className="update-notice-actions">
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            onOpen()
            if (status === 'available') void updater.install()
          }}
        >
          {status === 'available' ? t('update.downloadInstall') : t('update.viewDetails')}
        </Button>
        {!downloading && (
          <Button variant="ghost" size="sm" onClick={updater.dismissNotice}>
            {t('update.later')}
          </Button>
        )}
      </div>
    </aside>
  )
}
