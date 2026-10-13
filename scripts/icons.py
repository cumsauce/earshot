import math, struct, zlib, sys
def png(path, px, n=128):
    raw=b''.join(b'\x00'+bytes(sum((px(x,y) for x in range(n)),())) for y in range(n))
    def chunk(t,d): return struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
    open(path,'wb').write(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',n,n,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(raw,9))+chunk(b'IEND',b''))
def rounded(x,y,n=128,r=22):
    cx=min(max(x,r),n-1-r); cy=min(max(y,r),n-1-r)
    return math.hypot(x-cx,y-cy)<=r
def mix(a,b,t): return tuple(int(a[i]+(b[i]-a[i])*t) for i in range(3))
# Foley: a decaying waveform on deep teal
def foley(x,y):
    if not rounded(x,y): return (0,0,0,0)
    bg=mix((18,52,60),(10,30,38),y/127)
    t=(x-14)/100
    if 0<=t<=1:
        w=math.sin(t*2*math.pi*5)*math.exp(-2.6*t)*38
        d=abs((64-w)-y)
        if d<3.2: return mix((250,200,90),bg,max(0,d-2.2)) + (255,)
    return bg+(255,)
# Earshot: an ear-like point with sound arcs reflecting
def earshot(x,y):
    if not rounded(x,y): return (0,0,0,0)
    bg=mix((40,30,70),(20,16,40),y/127)
    dx,dy=x-34,y-64
    r=math.hypot(dx,dy); a=math.atan2(dy,dx)
    if r<9: return (240,240,255,255)
    if abs(a)<0.9:
        for k,rr in enumerate((26,44,62,80)):
            if abs(r-rr)<3: return mix((140,200,255),bg,k*0.18)+(255,)
    return bg+(255,)
png(sys.argv[1],foley); png(sys.argv[2],earshot)
