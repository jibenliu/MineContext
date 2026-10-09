// Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
// SPDX-License-Identifier: Apache-2.0

import { Button, Grid, Modal, Typography, Upload } from '@arco-design/web-react'
import { IconCheckCircleFill, IconClose } from '@arco-design/web-react/icon'
import uploadIcon from '@renderer/assets/images/files/upload.png'
import { useFiles } from '@renderer/hooks/use-files'
import { useI18n } from '@renderer/i18n'
import { typeIconMap } from '@renderer/utils/file'
import React, { useRef, useState } from 'react'

const { Title, Text } = Typography
const { Row, Col } = Grid

const Files: React.FC = () => {
  const { t } = useI18n()
  const [analyzeVisible, setAnalyzeVisible] = useState(false)
  const [selectedDoc, setSelectedDoc] = useState<any>(null)
  const [saving, setSaving] = useState(false)
  const [saveFailed, setSaveFailed] = useState(false)
  const savingRef = useRef(false)
  const { analyzedDocs, addFile, importFile, loadFiles, loading, loadFailed } = useFiles()

  const uploadFile = async (file) => {
    if (!file.originFile) {
      return
    }

    if (savingRef.current) return
    const fileType = file.name.split('.').pop()?.toLowerCase() ?? ''
    const newDoc = {
      name: file.name,
      source: `${fileType.toUpperCase()} · ${(file.originFile.size / 1024 / 1024).toFixed(2)}MB`,
      file: file.originFile,
      icon: typeIconMap[fileType] ?? typeIconMap.txt
    }
    setSelectedDoc(newDoc)
    setSaveFailed(false)
    setAnalyzeVisible(true)
  }

  const analyzeDocument = async () => {
    if (!selectedDoc?.file || savingRef.current) return
    savingRef.current = true
    setSaving(true)
    setSaveFailed(false)
    try {
      const fileData = await new Promise<ArrayBuffer>((resolve, reject) => {
        const reader = new FileReader()
        reader.onload = () => resolve(reader.result as ArrayBuffer)
        reader.onerror = () => reject(reader.error)
        reader.onabort = () => reject(new Error('Read aborted'))
        reader.readAsArrayBuffer(selectedDoc.file)
      })
      const result = await importFile(selectedDoc.name, new Uint8Array(fileData))
      if (!result?.id || !result.file_path) throw new Error('Import failed')
      addFile({
        name: selectedDoc.name,
        source: selectedDoc.source,
        icon: selectedDoc.icon,
        filePath: result.file_path,
        status: 'Analyzed',
        prompt: ''
      })
      setAnalyzeVisible(false)
    } catch {
      setSaveFailed(true)
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  return (
    <div className="mt-5 h-screen overflow-y-auto m-5 p-5 border-4 border-transparent rounded-[20px] bg-clip-padding-box relative before:content-[''] before:absolute before:top-0 before:left-0 before:right-0 before:bottom-0 before:z-[-1] before:m-[-4px] before:rounded-[24px] before:bg-gradient-to-br before:from-blue-400 before:via-purple-400 before:to-blue-400 p-5 bg-white h-full scrollbar-hide">
      <div className="bg-white rounded-2xl p-6 m-1 h-[calc(100%-8px)] shadow-[0_8px_32px_rgba(102,126,234,0.1)]">
        <div className="flex justify-between items-start mb-3 px-2 max-md:flex-col max-md:items-stretch">
          <div className="w-3/5 max-md:w-full">
            <Title heading={3} style={{ marginTop: 5, fontWeight: 700, fontSize: 24 }}>
              {t('files.title')}
            </Title>
            <Text type="secondary" style={{ width: 519, fontSize: 12 }}>
              {t('files.subtitle')}
            </Text>
          </div>
          <div className="flex items-center ml-6 max-md:ml-0 max-md:mt-4 max-md:justify-end"></div>
        </div>

        {/* Upload area */}
        <div className="mt-5">
          <Upload
            drag
            autoUpload={false}
            disabled={saving}
            accept=".pptx,.pdf,.docx,.xlsx,.csv,.txt,.md,.markdown,.faq,.html,.htm,.png,.jpg,.jpeg,.gif,.webp"
            showUploadList={false}
            onChange={(_, file) => {
              if (file.status === 'init') {
                uploadFile(file)
              }
            }}
            className="mb-0 max-w-[1200px] mx-auto flex-1 flex flex-col [&_.arco-upload-list]:hidden"
            style={{ height: '180px', fontSize: 12 }}>
            <div className="border-2 border-dashed border-[var(--color-border-2)] rounded-xl p-[30px] bg-[var(--color-fill-1)] transition-all duration-300 flex-1 flex flex-col max-h-[600px] h-[200px] cursor-pointer hover:border-[rgb(var(--primary-6))] hover:bg-[var(--color-primary-light-1)]">
              <div className="flex items-center justify-center flex-1">
                <div className="text-center flex flex-col items-center justify-center">
                  <img src={uploadIcon} alt="Screen recording" style={{ width: 214 }} />
                  <Text style={{ color: 'var(--color-text-1)', fontSize: 14, fontWeight: 700, marginTop: 10 }}>
                    {t('files.dropHint')}
                  </Text>
                  <Text style={{ color: 'var(--color-text-2)', fontSize: 13, marginTop: 6 }}>
                    {t('files.formatHint')}
                  </Text>
                </div>
              </div>
            </div>
          </Upload>
        </div>

        <div className="mt-[50px]">
          <Title heading={5} style={{ marginTop: 5, fontWeight: 700, fontSize: 24 }}>
            {t('files.analyzedDocuments')}
          </Title>
          {loading && <p role="status">{t('files.loading')}</p>}
          {loadFailed && (
            <div role="alert">
              {t('files.loadFailed')}
              <Button onClick={() => void loadFiles()}>{t('common.retry')}</Button>
            </div>
          )}
          {!loading && !loadFailed && analyzedDocs.length === 0 && <p>{t('files.empty')}</p>}
          <Row gutter={[24, 24]} style={{ marginTop: 20 }}>
            {analyzedDocs.map((doc) => (
              <Col span={8} key={doc.name}>
                <div className="bg-white rounded-lg p-4 items-center border border-[var(--color-border-2)] cursor-pointer hover:shadow-lg transition-shadow">
                  <div style={{ display: 'flex' }}>
                    <div style={{ marginRight: 8 }}>
                      <img
                        src={doc.icon}
                        alt="document icon"
                        style={{ width: 36, height: 36, maxWidth: 36, maxHeight: 36 }}
                      />
                    </div>
                    <div className="flex flex-col flex-grow overflow-hidden" style={{ overflow: 'hidden' }}>
                      <Text
                        style={{
                          fontSize: 12,
                          color: 'var(--color-text-1)',
                          whiteSpace: 'nowrap',
                          textOverflow: 'ellipsis',
                          overflow: 'hidden'
                        }}>
                        {doc.name}
                      </Text>
                      <Text
                        type="secondary"
                        style={{
                          fontSize: 12,
                          color: 'var(--color-text-2)',
                          whiteSpace: 'nowrap',
                          textOverflow: 'ellipsis',
                          overflow: 'hidden'
                        }}>
                        {doc.prompt}
                      </Text>
                    </div>
                  </div>
                  <div className="flex items-center text-xs mt-2 px-2 py-1 rounded-md w-fit text-[rgb(var(--success-6))] bg-[var(--color-success-light-1)]">
                    <IconCheckCircleFill style={{ marginRight: 6, color: 'rgb(var(--success-6))' }} />
                    <span>{doc.status === 'Analyzed' ? t('files.analysisSuccess') : t('files.uploaded')}</span>
                  </div>
                </div>
              </Col>
            ))}
          </Row>
        </div>
      </div>

      {/* Analyze document modal */}
      <Modal
        title={t('files.importTitle')}
        visible={analyzeVisible}
        autoFocus={false}
        focusLock={true}
        onCancel={() => {
          if (!savingRef.current) setAnalyzeVisible(false)
        }}
        footer={
          <>
            <Button disabled={saving} onClick={() => setAnalyzeVisible(false)} style={{ fontSize: 12 }}>
              {t('common.cancel')}
            </Button>
            <Button type="primary" loading={saving} onClick={() => analyzeDocument()}>
              {t('files.importAction')}
            </Button>
          </>
        }
        closeIcon={<IconClose style={{ fontSize: 20, color: 'var(--color-text-2)' }} />}
        className="[&_.arco-modal-title]:font-semibold">
        {selectedDoc && (
          <div>
            <div className="bg-[var(--color-fill-1)] rounded-md p-3 flex items-center border border-[var(--color-border-2)]">
              <div style={{ marginRight: 12 }}>
                <img
                  src={selectedDoc.icon}
                  alt="document icon"
                  style={{ width: 36, height: 36, maxWidth: 36, maxHeight: 36 }}
                />
              </div>
              <div className="flex flex-col">
                <Text style={{ fontSize: 12, color: 'var(--color-text-1)' }}>{selectedDoc.name}</Text>
                <Text style={{ fontSize: 12, color: 'var(--color-text-2)' }}>{selectedDoc.source}</Text>
              </div>
            </div>
            <p className="mt-4">{t('files.storageHint')}</p>
            {saveFailed && <p role="alert">{t('files.saveFailed')}</p>}
          </div>
        )}
      </Modal>
    </div>
  )
}

export default Files
