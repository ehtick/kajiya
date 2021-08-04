use kajiya_backend::{ash::vk, vulkan::image::*};
use kajiya_rg::{self as rg};
use rg::{RenderGraph, SimpleRenderPass};

pub fn blur_pyramid(rg: &mut RenderGraph, input: &rg::Handle<Image>) -> rg::Handle<Image> {
    let skip_n_bottom_mips = 1;
    let mut pyramid_desc = input
        .desc()
        .half_res()
        .format(vk::Format::B10G11R11_UFLOAT_PACK32) // R16G16B16A16_SFLOAT
        .all_mip_levels();
    pyramid_desc.mip_levels = (pyramid_desc
        .mip_levels
        .overflowing_sub(skip_n_bottom_mips)
        .0)
        .max(1);

    let mut output = rg.create(pyramid_desc);

    SimpleRenderPass::new_compute(rg.add_pass("_blur0"), "/shaders/blur.hlsl")
        .read(input)
        .write_view(
            &mut output,
            ImageViewDesc::builder()
                .base_mip_level(0)
                .level_count(Some(1)),
        )
        .dispatch(output.desc().extent);

    for target_mip in 1..(output.desc().mip_levels as u32) {
        let downsample_amount = 1 << target_mip;

        SimpleRenderPass::new_compute(
            rg.add_pass(&format!("_blur{}", target_mip)),
            "/shaders/blur.hlsl",
        )
        .read_view(
            &output,
            ImageViewDesc::builder()
                .base_mip_level(target_mip - 1)
                .level_count(Some(1)),
        )
        .write_view(
            &mut output,
            ImageViewDesc::builder()
                .base_mip_level(target_mip)
                .level_count(Some(1)),
        )
        .dispatch(
            output
                .desc()
                .div_extent([downsample_amount, downsample_amount, 1])
                .extent,
        );
    }

    output
}

pub fn rev_blur_pyramid(rg: &mut RenderGraph, in_pyramid: &rg::Handle<Image>) -> rg::Handle<Image> {
    let mut output = rg.create(*in_pyramid.desc());

    for target_mip in (0..(output.desc().mip_levels as u32 - 1)).rev() {
        let downsample_amount = 1 << target_mip;
        let output_extent: [u32; 3] = output
            .desc()
            .div_extent([downsample_amount, downsample_amount, 1])
            .extent;
        let src_mip: u32 = target_mip + 1;
        let self_weight = if src_mip == output.desc().mip_levels as u32 {
            0.0f32
        } else {
            0.5f32
        };

        SimpleRenderPass::new_compute(
            rg.add_pass(&format!("_rev_blur{}", target_mip)),
            "/shaders/rev_blur.hlsl",
        )
        .read_view(
            &in_pyramid,
            ImageViewDesc::builder()
                .base_mip_level(target_mip)
                .level_count(Some(1)),
        )
        .read_view(
            &output,
            ImageViewDesc::builder()
                .base_mip_level(src_mip)
                .level_count(Some(1)),
        )
        .write_view(
            &mut output,
            ImageViewDesc::builder()
                .base_mip_level(target_mip)
                .level_count(Some(1)),
        )
        .constants((output_extent[0], output_extent[1], self_weight))
        .dispatch(output_extent);
    }

    output
}

#[allow(dead_code)]
fn edge_preserving_filter_luminance(
    rg: &mut RenderGraph,
    input: &rg::Handle<Image>,
) -> rg::Handle<Image> {
    let mut lum_tex = rg.create(input.desc().format(vk::Format::R16G16_SFLOAT));

    SimpleRenderPass::new_compute(
        rg.add_pass("log luminance"),
        "/shaders/tonemap/extract_log_luminance.hlsl",
    )
    .read(&input)
    .write(&mut lum_tex)
    .dispatch(lum_tex.desc().extent);

    let mut input = &lum_tex;
    let mut input_next: Option<_> = None;

    for i in 0..7 {
        let mut output = rg.create(input.desc().format(vk::Format::R16G16_SFLOAT));
        let px_skip: u32 = 1 << (6 - i);

        SimpleRenderPass::new_compute(
            rg.add_pass("a-trous"),
            "/shaders/tonemap/luminance-a-trous.hlsl",
        )
        .read(&input)
        .read(&lum_tex)
        .write(&mut output)
        .constants((input.desc().extent_inv_extent_2d(), px_skip))
        .dispatch(output.desc().extent);

        input_next = Some(output);
        input = input_next.as_ref().unwrap();
    }

    input_next.unwrap()
}

pub fn post_process(
    rg: &mut RenderGraph,
    input: &rg::Handle<Image>,
    //debug_input: &rg::Handle<Image>,
    bindless_descriptor_set: vk::DescriptorSet,
    ev_shift: f32,
) -> rg::Handle<Image> {
    let blur_pyramid = blur_pyramid(rg, input);
    let rev_blur_pyramid = rev_blur_pyramid(rg, &blur_pyramid);

    let mut output = rg.create(input.desc().format(vk::Format::B10G11R11_UFLOAT_PACK32));
    let blur_pyramid_mip_count: u32 = blur_pyramid.desc().mip_levels as _;

    //let blurred_luminance = edge_preserving_filter_luminance(rg, input);

    SimpleRenderPass::new_compute(rg.add_pass("post combine"), "/shaders/post_combine.hlsl")
        .read(input)
        //.read(debug_input)
        .read(&blur_pyramid)
        .read(&rev_blur_pyramid)
        //.read(&blurred_luminance)
        .write(&mut output)
        .raw_descriptor_set(1, bindless_descriptor_set)
        .constants((
            output.desc().extent_inv_extent_2d(),
            blur_pyramid_mip_count,
            ev_shift,
        ))
        .dispatch(output.desc().extent);

    output
}
