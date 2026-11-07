import { createBlobMatrices, createIcosphere } from "./math";

export interface BlobRenderer {
  render(values: { time: number; intensity: number; warmth: number; dim: number; level: number; theme: "light" | "dark" }): void;
  resize(width: number, height: number, dpr: number): void;
  dispose(): void;
}

const noise = `
vec3 mod289(vec3 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 mod289(vec4 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 permute(vec4 x){return mod289(((x*34.0)+1.0)*x);}
vec4 taylorInvSqrt(vec4 r){return 1.79284291400159-0.85373472095314*r;}
float cnoise(vec3 P){
 vec3 Pi0=floor(P), Pi1=Pi0+vec3(1.0); Pi0=mod289(Pi0); Pi1=mod289(Pi1);
 vec3 Pf0=fract(P), Pf1=Pf0-vec3(1.0);
 vec4 ix=vec4(Pi0.x,Pi1.x,Pi0.x,Pi1.x), iy=vec4(Pi0.yy,Pi1.yy);
 vec4 iz0=Pi0.zzzz, iz1=Pi1.zzzz;
 vec4 ixy=permute(permute(ix)+iy), ixy0=permute(ixy+iz0), ixy1=permute(ixy+iz1);
 vec4 gx0=ixy0*(1.0/7.0); vec4 gy0=fract(floor(gx0)*(1.0/7.0))-0.5; gx0=fract(gx0); vec4 gz0=vec4(0.5)-abs(gx0)-abs(gy0); vec4 sz0=step(gz0,vec4(0.0)); gx0-=sz0*(step(0.0,gx0)-0.5); gy0-=sz0*(step(0.0,gy0)-0.5);
 vec4 gx1=ixy1*(1.0/7.0); vec4 gy1=fract(floor(gx1)*(1.0/7.0))-0.5; gx1=fract(gx1); vec4 gz1=vec4(0.5)-abs(gx1)-abs(gy1); vec4 sz1=step(gz1,vec4(0.0)); gx1-=sz1*(step(0.0,gx1)-0.5); gy1-=sz1*(step(0.0,gy1)-0.5);
 vec3 g000=vec3(gx0.x,gy0.x,gz0.x), g100=vec3(gx0.y,gy0.y,gz0.y), g010=vec3(gx0.z,gy0.z,gz0.z), g110=vec3(gx0.w,gy0.w,gz0.w);
 vec3 g001=vec3(gx1.x,gy1.x,gz1.x), g101=vec3(gx1.y,gy1.y,gz1.y), g011=vec3(gx1.z,gy1.z,gz1.z), g111=vec3(gx1.w,gy1.w,gz1.w);
 vec4 norm0=taylorInvSqrt(vec4(dot(g000,g000),dot(g010,g010),dot(g100,g100),dot(g110,g110))); g000*=norm0.x;g010*=norm0.y;g100*=norm0.z;g110*=norm0.w;
 vec4 norm1=taylorInvSqrt(vec4(dot(g001,g001),dot(g011,g011),dot(g101,g101),dot(g111,g111))); g001*=norm1.x;g011*=norm1.y;g101*=norm1.z;g111*=norm1.w;
 float n000=dot(g000,Pf0), n100=dot(g100,vec3(Pf1.x,Pf0.yz)), n010=dot(g010,vec3(Pf0.x,Pf1.y,Pf0.z)), n110=dot(g110,vec3(Pf1.xy,Pf0.z));
 float n001=dot(g001,vec3(Pf0.xy,Pf1.z)), n101=dot(g101,vec3(Pf1.x,Pf0.y,Pf1.z)), n011=dot(g011,vec3(Pf0.x,Pf1.yz)), n111=dot(g111,Pf1);
 vec3 fade_xyz=Pf0*Pf0*Pf0*(Pf0*(Pf0*6.0-15.0)+10.0); vec4 n_z=mix(vec4(n000,n100,n010,n110),vec4(n001,n101,n011,n111),fade_xyz.z); vec2 n_yz=mix(n_z.xy,n_z.zw,fade_xyz.y); return 2.2*mix(n_yz.x,n_yz.y,fade_xyz.x);
}`;

function sources(webgl2: boolean) {
  const header = webgl2 ? "#version 300 es\n" : "";
  const vertex = `${header}${webgl2 ? "in" : "attribute"} vec3 aPosition;\n${webgl2 ? "in" : "attribute"} vec3 aNormal;\n${webgl2 ? "in" : "attribute"} vec2 aUv;\nuniform mat4 uProjection,uView;uniform float uTime,uIntensity,uLevel,uSpeed;\n${webgl2 ? "out" : "varying"} vec3 vNormal;${webgl2 ? "out" : "varying"} vec3 vView;${webgl2 ? "out" : "varying"} vec2 vUv;${webgl2 ? "out" : "varying"} float vDisplacement;\n${noise}\nvoid main(){float n=cnoise(aPosition*2.45+vec3(uTime*uSpeed*0.34,uTime*uSpeed*0.2,-uTime*uSpeed*0.17));vDisplacement=n*0.5+0.5;float d=n*uIntensity*0.62;vec3 p=aPosition+aNormal*d;p*=1.0+uLevel*0.025;vec4 viewPos=uView*vec4(p,1.0);vNormal=mat3(uView)*aNormal;vView=-viewPos.xyz;vUv=aUv;gl_Position=uProjection*viewPos;}`;
  const fragment = `${header}precision highp float;${webgl2 ? "in" : "varying"} vec3 vNormal;${webgl2 ? "in" : "varying"} vec3 vView;${webgl2 ? "in" : "varying"} vec2 vUv;${webgl2 ? "in" : "varying"} float vDisplacement;uniform vec3 uColor,uHighlight,uAccent;uniform float uIntensity,uTime,uWarmth,uDim;${webgl2 ? "out vec4 outColor;" : ""}\nvoid main(){vec3 N=normalize(vNormal);vec3 V=normalize(vView);float band=max(0.0,2.0*vDisplacement*uIntensity*sin(vUv.y*10.0+uTime*0.8));vec3 color=mix(uColor,uHighlight,clamp(band,0.0,0.82));float fresnel=pow(1.0-max(dot(N,V),0.0),2.7);vec3 L=normalize(vec3(-0.42,0.72,0.8));float spec=pow(max(dot(reflect(-L,N),V),0.0),34.0)*0.22;float diffuse=0.74+0.26*max(dot(N,L),0.0);vec3 lit=color*diffuse+uAccent*fresnel*0.48+vec3(1.0)*spec;float alpha=(0.91+fresnel*0.08)*(1.0-uDim);${webgl2 ? "outColor" : "gl_FragColor"}=vec4(lit*alpha,alpha);}`;
  return { vertex, fragment };
}

function compile(gl: WebGLRenderingContext | WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader=gl.createShader(type); if(!shader) throw new Error("Unable to create blob shader");
  gl.shaderSource(shader,source);gl.compileShader(shader);if(!gl.getShaderParameter(shader,gl.COMPILE_STATUS)){const message=gl.getShaderInfoLog(shader)??"Shader compilation failed";gl.deleteShader(shader);throw new Error(message);}return shader;
}

export function createBlobRenderer(canvas: HTMLCanvasElement): BlobRenderer | null {
  const options={alpha:true,premultipliedAlpha:true,antialias:true};
  const gl=(canvas.getContext("webgl2",options) as WebGL2RenderingContext|null)??(canvas.getContext("webgl",options) as WebGLRenderingContext|null);
  if(!gl)return null;
  const webgl2=typeof WebGL2RenderingContext!=="undefined"&&gl instanceof WebGL2RenderingContext;
  const {vertex,fragment}=sources(webgl2);let vs:WebGLShader|undefined,fs:WebGLShader|undefined,program:WebGLProgram|undefined;
  try {vs=compile(gl,gl.VERTEX_SHADER,vertex);fs=compile(gl,gl.FRAGMENT_SHADER,fragment);program=gl.createProgram()??undefined;if(!program)throw new Error("Unable to create blob program");gl.attachShader(program,vs);gl.attachShader(program,fs);gl.linkProgram(program);if(!gl.getProgramParameter(program,gl.LINK_STATUS))throw new Error(gl.getProgramInfoLog(program)??"Shader link failed");}
  catch {if(vs)gl.deleteShader(vs);if(fs)gl.deleteShader(fs);if(program)gl.deleteProgram(program);return null;}
  const sphere=createIcosphere(4), buffers:WebGLBuffer[]=[];
  const buffer=(target:number,data:BufferSource) => {const b=gl.createBuffer();if(!b)throw new Error("Unable to create blob geometry buffer");buffers.push(b);gl.bindBuffer(target,b);gl.bufferData(target,data,gl.STATIC_DRAW);return b;};
  buffer(gl.ARRAY_BUFFER,sphere.positions as unknown as BufferSource);buffer(gl.ARRAY_BUFFER,sphere.normals as unknown as BufferSource);buffer(gl.ARRAY_BUFFER,sphere.uvs as unknown as BufferSource);
  const indexType=sphere.indices instanceof Uint32Array?gl.UNSIGNED_INT:gl.UNSIGNED_SHORT;
  if(indexType===gl.UNSIGNED_INT&&!webgl2) gl.getExtension("OES_element_index_uint");
  buffer(gl.ELEMENT_ARRAY_BUFFER,sphere.indices as unknown as BufferSource);
  const attributes=[gl.getAttribLocation(program,"aPosition"),gl.getAttribLocation(program,"aNormal"),gl.getAttribLocation(program,"aUv")];
  const uniforms=Object.fromEntries(["uProjection","uView","uTime","uIntensity","uLevel","uSpeed","uColor","uHighlight","uAccent","uWarmth","uDim"].map(k=>[k,gl.getUniformLocation(program!,k)]));
  const matrices=createBlobMatrices(1);
  const dark={body:[0.008,0.012,0.021],highlight:[0.34,0.56,0.68],accent:[0.42,0.68,0.84]};
  const light={body:[0.74,0.81,0.84],highlight:[0.24,0.40,0.51],accent:[0.35,0.55,0.66]};
  const tint=(base:number[],warmth:number,theme:"light"|"dark")=>{
    const stops=theme==="dark"?[[0.12,[0.42,0.69,0.82]],[0.44,[0.65,0.48,0.91]],[0.72,[0.88,0.58,0.42]],[1,[0.72,0.3,0.25]]]:[[0.12,[0.34,0.55,0.66]],[0.44,[0.48,0.35,0.7]],[0.72,[0.68,0.43,0.32]],[1,[0.65,0.29,0.24]]];
    let lower=0, previous=base;for(const [at,color] of stops as [number,number[]][]){if(warmth<=at){const amount=Math.max(0,Math.min(1,(warmth-lower)/(at-lower)));return previous.map((v,i)=>v+(color[i]-v)*amount);}lower=at;previous=color;}return previous;
  };
  const set=(name:string,value:number)=>gl.uniform1f(uniforms[name],value);
  return {
    resize(width,height,dpr){canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);gl.viewport(0,0,canvas.width,canvas.height);const m=createBlobMatrices(width/Math.max(height,1));gl.useProgram(program!);gl.uniformMatrix4fv(uniforms.uProjection,false,m.projection);gl.uniformMatrix4fv(uniforms.uView,false,m.view);},
    render(v){gl.useProgram(program!);gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);gl.enable(gl.DEPTH_TEST);gl.enable(gl.CULL_FACE);gl.enable(gl.BLEND);gl.blendFunc(gl.ONE,gl.ONE_MINUS_SRC_ALPHA);gl.bindBuffer(gl.ARRAY_BUFFER,buffers[0]);gl.enableVertexAttribArray(attributes[0]);gl.vertexAttribPointer(attributes[0],3,gl.FLOAT,false,0,0);gl.bindBuffer(gl.ARRAY_BUFFER,buffers[1]);gl.enableVertexAttribArray(attributes[1]);gl.vertexAttribPointer(attributes[1],3,gl.FLOAT,false,0,0);gl.bindBuffer(gl.ARRAY_BUFFER,buffers[2]);gl.enableVertexAttribArray(attributes[2]);gl.vertexAttribPointer(attributes[2],2,gl.FLOAT,false,0,0);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,buffers[3]);
      const pal=v.theme==="dark"?dark:light;const warmth=Math.min(1,v.warmth);const body=pal.body.map((x,i)=>x+(tint(pal.body,warmth,v.theme)[i]-x)*Math.min(0.2,warmth));const highlight=tint(pal.highlight,warmth,v.theme);const accent=tint(pal.accent,warmth,v.theme);
      gl.uniform3fv(uniforms.uColor,body);gl.uniform3fv(uniforms.uHighlight,highlight);gl.uniform3fv(uniforms.uAccent,accent);set("uTime",v.time);set("uIntensity",v.intensity);set("uLevel",v.level);set("uSpeed",1);set("uWarmth",v.warmth);set("uDim",v.dim);gl.drawElements(gl.TRIANGLES,sphere.indices.length,indexType,0);},
    dispose(){for(const b of buffers)gl.deleteBuffer(b);gl.deleteProgram(program!);gl.deleteShader(vs!);gl.deleteShader(fs!);},
  };
}
