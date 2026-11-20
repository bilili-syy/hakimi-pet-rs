#!/usr/bin/env python3
"""立绘有损压缩：RGBA → 256 色调色板 PNG（每色带透明度，tRNS）。

用法：python tools/quantize_sprites.py            # 处理 assets/ 下所有非调色板立绘（跳过 icon.png、tray.png 和 sprites/live 贴片）
之后跑 oxipng -o4 --strip safe -r assets 再无损压一遍。需要 numpy + Pillow（只在开发机用，不进 exe）。
算法：预乘 RGBA 空间按像素数加权 k-means（256 色，0 号留给全透明），不抖动；PSNR 约 37dB。
"""
import os,sys,glob,numpy as np
from PIL import Image
def kq(im,K=256,it=12,seed=0):
    a=np.asarray(im.convert('RGBA')).reshape(-1,4).astype(np.float32)
    # 全透明像素统一成 (0,0,0,0)，不参与聚类
    vis=a[:,3]>0
    x=a[vis].copy(); x[:,:3]*=x[:,3:4]/255  # 预乘：半透明像素颜色差按可见程度算
    u,inv,cnt=np.unique(x.round().astype(np.int16),axis=0,return_inverse=True,return_counts=True)
    u=u.astype(np.float32); w=cnt.astype(np.float32); inv=inv.reshape(-1)
    rng=np.random.default_rng(seed)
    K1=K-1
    c=u[rng.choice(len(u),min(K1,len(u)),replace=False,p=w/w.sum())] if len(u)>K1 else u
    def assign(c):
        lab=np.empty(len(u),np.int32)
        cc=(c**2).sum(1)
        for i in range(0,len(u),20000):
            b=u[i:i+20000]; d=cc[None]-2*b@c.T; lab[i:i+20000]=d.argmin(1)
        return lab
    for _ in range(it):
        lab=assign(c)
        s=np.zeros_like(c); n=np.zeros(len(c),np.float32)
        np.add.at(s,lab,u*w[:,None]); np.add.at(n,lab,w)
        empty=n==0
        c[~empty]=s[~empty]/n[~empty,None]
        if empty.any():  # 空簇挪到误差最大的点
            err=((u-c[lab])**2).sum(1)*w; c[empty]=u[np.argsort(-err)[:empty.sum()]]
    lab=assign(c)
    pal=c.copy(); al=np.clip(pal[:,3],1,255); pal[:,:3]=np.clip(pal[:,:3]*255/al[:,None],0,255)
    idx=np.zeros(a.shape[0],np.uint8); idx[vis]=(lab[inv]+1).astype(np.uint8)
    P=Image.fromarray(idx.reshape(im.size[1],im.size[0]),'P')
    flat=[0,0,0]+[int(round(v)) for v in pal[:,:3].reshape(-1)]
    flat+= [0]*(768-len(flat)); P.putpalette(flat)
    P.info['transparency']=bytes([0]+[int(round(v)) for v in pal[:,3]]+[255]*(255-len(pal)))
    return P
def _one(src,dst):
    im=Image.open(src); q=kq(im); q.save(dst,transparency=q.info['transparency'],optimize=True)
    a=np.asarray(im.convert('RGBA')).astype(np.float32); b=np.asarray(Image.open(dst).convert('RGBA')).astype(np.float32)
    mse=((a[...,:3]*a[...,3:]/255-b[...,:3]*b[...,3:]/255)**2).mean()
    return 10*np.log10(65025/max(mse,1e-9))

if __name__=='__main__':
    root=os.path.join(os.path.dirname(os.path.abspath(__file__)),'..','assets')
    for p in sorted(glob.glob(os.path.join(root,'**','*.png'),recursive=True)):
        rel=os.path.relpath(p,root).replace(os.sep,'/')
        if rel in ('icon.png','tray.png') or rel.startswith('sprites/live/') or Image.open(p).mode=='P': continue
        n0=os.path.getsize(p); ps=_one(p,p)
        print('%-36s %8d -> %8d  psnr %.1f'%(rel,n0,os.path.getsize(p),ps))
