// Shared scene-linear Rec.2020 channel/selective/skin color correction helpers.
// Included in global tone and local retouch shaders; not a standalone pipeline.
struct PhotographicColor {
    mixer: array<vec4<f32>,3>,
    selective: array<vec4<f32>,9>,
    skin: vec4<f32>,
    control: vec4<f32>,
}

fn photographic_color(input:vec3<f32>, settings:PhotographicColor)->vec3<f32> {
    if settings.control.w < 0.5 {return input;}
    var rgb=max(vec3<f32>(dot(settings.mixer[0].xyz,input),dot(settings.mixer[1].xyz,input),dot(settings.mixer[2].xyz,input)),vec3<f32>(0.0));
    if settings.control.z > 0.5 {
        let color=max(REC2020_TO_REC709*rgb,vec3<f32>(0.0));
        let high=max(max(color.r,color.g),color.b);let low=min(min(color.r,color.g),color.b);
        let delta=high-low;let saturation=delta/max(high,1e-6);
        var hue=0.0;
        if delta > 1e-6 {
            if high == color.r {hue=((color.g-color.b)/delta+6.0)%6.0*60.0;}
            else if high == color.g {hue=((color.b-color.r)/delta+2.0)*60.0;}
            else {hue=((color.r-color.g)/delta+4.0)*60.0;}
        }
        let y=dot(color,vec3<f32>(0.2126,0.7152,0.0722));
        let chromatic=smoothstep(0.05,0.25,saturation);
        let white=smoothstep(0.35,0.85,y)*(1.0-chromatic);
        let black=(1.0-smoothstep(0.02,0.25,y))*(1.0-chromatic);
        var correction=vec4<f32>(0.0);
        for(var band=0u;band<9u;band++) {
            var weight=0.0;
            if band==6u {weight=white;}
            else if band==7u {weight=(1.0-white-black)*(1.0-chromatic);}
            else if band==8u {weight=black;}
            else {
                let difference=(hue-f32(band)*60.0+540.0)%360.0-180.0;
                weight=max(1.0-abs(difference)/60.0,0.0)*chromatic;
            }
            correction+=settings.selective[band]*weight*0.01;
        }
        let scale=select(vec3<f32>(1.0),color,settings.control.y>0.5);
        rgb=REC709_TO_REC2020*max(color-correction.xyz*scale-correction.w*color,vec3<f32>(0.0));
    }
    if settings.skin.z > 0.0 || settings.skin.w > 0.0 {
        let linear709=max(REC2020_TO_REC709*rgb,vec3<f32>(0.0));
        let lab=OK_M2*pow(max(OK_M1*linear709,vec3<f32>(0.0)),vec3<f32>(1.0/3.0));
        let c=length(lab.yz);let h=atan2(lab.z,lab.y)*57.29577951;
        let difference=(h-settings.skin.x+900.0)%360.0-180.0;
        let weight=(1.0-smoothstep(settings.skin.y*0.5,settings.skin.y,abs(difference)))*smoothstep(0.01,0.04,c);
        let angle=(h-difference*weight*settings.skin.z*0.01)*0.01745329;
        let chroma=mix(c,settings.control.x,weight*settings.skin.w*0.01);
        let lms=OK_M2_INV*vec3<f32>(lab.x,chroma*cos(angle),chroma*sin(angle));
        rgb=REC709_TO_REC2020*(OK_M1_INV*(lms*lms*lms));
    }
    return max(rgb,vec3<f32>(0.0));
}
