mod boilerplate;
mod debug;
mod material;
mod mesh;
mod primitives;
mod render_object;

use std::{
    cell::RefCell,
    collections::HashMap,
    mem::{size_of, ManuallyDrop},
    rc::Rc,
};

use ash::{
    vk::{
        self, AccessFlags, AttachmentDescription, AttachmentLoadOp, AttachmentStoreOp,
        BufferCreateInfo, BufferUsageFlags, ClearValue, Framebuffer, FramebufferCreateInfo,
        ImageLayout, PipelineStageFlags, Rect2D, RenderPass, RenderPassCreateInfo,
        SampleCountFlags, SubpassDependency, SubpassDescription, SUBPASS_EXTERNAL,
    },
    Device,
};
use log::trace;

use vk_mem::{Alloc, AllocationCreateInfo, Allocator};

use crate::{
    config::Config,
    engine::renderer::{mesh::MeshPushConstants, primitives::AllocatedBuffer},
};

use boilerplate::Boilerplate;

use primitives::Pipeline;
use primitives::Shader;
use primitives::Swapchain;

use material::Material;

use mesh::Mesh;
use mesh::Vertex;

use render_object::RenderObject;

pub struct Renderer {
    framenumber: u64,
    boilerplate: Boilerplate,
    render_pass: RenderPass,
    framebuffers: Vec<Framebuffer>,
    render_semaphore: vk::Semaphore,
    present_semaphore: vk::Semaphore,
    fence: vk::Fence,
    mesh_pipeline: ManuallyDrop<Pipeline>,
    meshes: HashMap<String, Rc<RefCell<Mesh>>>,
    materials: HashMap<String, Rc<RefCell<Material>>>,
    renderables: Vec<RenderObject>,
}

impl Renderer {
    pub fn new(_config: Config, window: &winit::window::Window) -> Result<Renderer, String> {
        trace!("Initializing: Renderer");

        let mut boilerplate = match Boilerplate::new(_config, window) {
            Ok(boilerplate) => boilerplate,
            Err(e) => return Err("Failed to init boilerplate: ".to_owned() + &e),
        };

        let render_pass = match Self::init_render_pass(&boilerplate.device, &boilerplate.swapchain)
        {
            Ok(render_pass) => render_pass,
            Err(e) => return Err("Failed to init renderer: render_pass: ".to_owned() + &e),
        };

        let framebuffers = match Self::init_frame_buffers(
            &boilerplate.device,
            &boilerplate.swapchain,
            &render_pass,
        ) {
            Ok(framebuffers) => framebuffers,
            Err(e) => return Err("Failed to init renderer: framebuffers: ".to_owned() + &e),
        };

        let vertex_shader =
            match Shader::from_path(&boilerplate.device, "assets/shaders/tri_mesh.vert") {
                Ok(shader) => shader,
                Err(e) => {
                    return Err("Failed to create vertex shader: ".to_owned() + &e.to_string())
                }
            };

        let color_fragment_shader =
            match Shader::from_path(&boilerplate.device, "assets/shaders/colored_triangle.frag") {
                Ok(shader) => shader,
                Err(e) => {
                    return Err("Failed to create fragment shader: ".to_owned() + &e.to_string())
                }
            };

        let mesh_pipeline = match Pipeline::new(
            &boilerplate.device,
            &[&vertex_shader, &color_fragment_shader],
            &render_pass,
            boilerplate.swapchain.extent.width,
            boilerplate.swapchain.extent.height,
            &Vertex::get_vertex_input_description(),
        ) {
            Ok(pipeline) => pipeline,
            Err(e) => return Err("Failed to create pipeline: ".to_owned() + &e.to_string()),
        };

        let fence_create_info =
            vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED);

        let fence = match unsafe { boilerplate.device.create_fence(&fence_create_info, None) } {
            Ok(fence) => fence,
            Err(e) => return Err("Failed to create fence: ".to_owned() + &e.to_string()),
        };

        let semaphore_create_info = vk::SemaphoreCreateInfo::builder().build();

        let render_semaphore = match unsafe {
            boilerplate
                .device
                .create_semaphore(&semaphore_create_info, None)
        } {
            Ok(semaphore) => semaphore,
            Err(e) => return Err("Failed to create semaphore: ".to_owned() + &e.to_string()),
        };

        let present_semaphore = match unsafe {
            boilerplate
                .device
                .create_semaphore(&semaphore_create_info, None)
        } {
            Ok(semaphore) => semaphore,
            Err(e) => return Err("Failed to create semaphore: ".to_owned() + &e.to_string()),
        };

        let mesh = Self::init_mesh(&mut boilerplate.allocator);
        let mut meshes = HashMap::new();
        meshes.insert("monkey".to_string(), Rc::new(RefCell::new(mesh)));

        Ok(Renderer {
            framenumber: 0,
            boilerplate,
            render_pass,
            framebuffers,
            fence,
            render_semaphore,
            present_semaphore,
            mesh_pipeline: ManuallyDrop::new(mesh_pipeline),
            meshes,
            renderables: Vec::new(),
            materials: HashMap::new(),
        })
    }

    fn init_render_pass(device: &Device, swapchain: &Swapchain) -> Result<RenderPass, String> {
        trace!("Initializing: Vk RenderPass");

        let attachment_description = AttachmentDescription::builder()
            .format(swapchain.image_format)
            .samples(SampleCountFlags::TYPE_1)
            .load_op(AttachmentLoadOp::CLEAR)
            .store_op(AttachmentStoreOp::STORE)
            .stencil_load_op(AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(AttachmentStoreOp::DONT_CARE)
            .initial_layout(ImageLayout::UNDEFINED)
            .final_layout(ImageLayout::PRESENT_SRC_KHR)
            .build();

        let attachment_references = [vk::AttachmentReference::builder()
            .attachment(0)
            .layout(ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .build()];

        let depth_attachment_description = AttachmentDescription::builder()
            .format(vk::Format::D32_SFLOAT)
            .samples(SampleCountFlags::TYPE_1)
            .load_op(AttachmentLoadOp::CLEAR)
            .store_op(AttachmentStoreOp::STORE)
            .stencil_load_op(AttachmentLoadOp::CLEAR)
            .stencil_store_op(AttachmentStoreOp::DONT_CARE)
            .initial_layout(ImageLayout::UNDEFINED)
            .final_layout(ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .build();

        let depth_attachment_references = vk::AttachmentReference::builder()
            .attachment(1)
            .layout(ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .build();

        let subpass_descriptions = [SubpassDescription::builder()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&attachment_references)
            .depth_stencil_attachment(&depth_attachment_references)
            .build()];

        let color_dependency = SubpassDependency::builder()
            .src_subpass(SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(AccessFlags::NONE)
            .dst_stage_mask(PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(AccessFlags::COLOR_ATTACHMENT_WRITE)
            .build();

        let depth_dependency = SubpassDependency::builder()
            .src_subpass(SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                PipelineStageFlags::EARLY_FRAGMENT_TESTS | PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
            .src_access_mask(AccessFlags::NONE)
            .dst_stage_mask(
                PipelineStageFlags::EARLY_FRAGMENT_TESTS | PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
            .dst_access_mask(AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
            .build();

        let render_pass_create_info = RenderPassCreateInfo::builder()
            .attachments(&[attachment_description, depth_attachment_description])
            .subpasses(&subpass_descriptions)
            .dependencies(&[color_dependency, depth_dependency])
            .build();

        match unsafe { device.create_render_pass(&render_pass_create_info, None) } {
            Ok(render_pass) => Ok(render_pass),
            Err(e) => Err("Failed to create render pass: ".to_owned() + &e.to_string()),
        }
    }

    fn init_frame_buffers(
        device: &Device,
        swapchain: &Swapchain,
        render_pass: &RenderPass,
    ) -> Result<Vec<Framebuffer>, String> {
        trace!("Initializing: Vk Framebuffers");

        let mut framebuffers: Vec<Framebuffer> = Vec::with_capacity(swapchain.image_views.len());

        for image_view in &swapchain.image_views {
            let attachments = [*image_view, swapchain.depth_image_view];

            let framebuffer_create_info = FramebufferCreateInfo::builder()
                .render_pass(*render_pass)
                .width(swapchain.extent.width)
                .height(swapchain.extent.height)
                .layers(1)
                .attachments(&attachments)
                .build();

            let framebuffer = match unsafe {
                device.create_framebuffer(&framebuffer_create_info, None)
            } {
                Ok(framebuffer) => framebuffer,
                Err(e) => return Err("Failed to create framebuffer: ".to_owned() + &e.to_string()),
            };

            framebuffers.push(framebuffer);
        }

        Ok(framebuffers)
    }

    fn init_mesh(allocator: &mut Allocator) -> Mesh {
        trace!("Initializing: Mesh");

        let mut mesh = Mesh::from_path("assets/models/monkey/monkey.glb");

        Self::upload_mesh(allocator, &mut mesh);

        mesh
    }

    fn upload_mesh(allocator: &mut Allocator, mesh: &mut Mesh) {
        trace!("Uploading: Mesh");

        let (buffer, mut allocation) = unsafe {
            // TODO: Figure out the right way to set memory usage since CpuToGpu is deprecated
            #[allow(deprecated)]
            allocator
                .create_buffer(
                    &BufferCreateInfo::builder()
                        .size((mesh.vertices.len() * size_of::<Vertex>()) as u64)
                        .usage(BufferUsageFlags::VERTEX_BUFFER)
                        .build(),
                    &AllocationCreateInfo {
                        usage: vk_mem::MemoryUsage::CpuToGpu,
                        ..Default::default()
                    },
                )
                .unwrap()
        };

        let memory_handle = unsafe { allocator.map_memory(&mut allocation).unwrap() };
        unsafe {
            std::ptr::copy_nonoverlapping(
                mesh.vertices.as_ptr() as *const u8,
                memory_handle,
                (mesh.vertices.len() * size_of::<Vertex>()) as usize,
            );
        }
        unsafe { allocator.unmap_memory(&mut allocation) };

        let allocated_buffer = AllocatedBuffer { buffer, allocation };

        mesh.vertex_buffer = Some(allocated_buffer);

        // Clear out the vertices data since we've now uploaded it to the GPU
        mesh.vertices = vec![];
    }

    pub fn render(&mut self) {
        trace!("Rendering");

        unsafe {
            self.boilerplate
                .device
                .wait_for_fences(&[self.fence], true, 1000000000)
        }
        .expect("Failed to wait for fence");

        unsafe { self.boilerplate.device.reset_fences(&[self.fence]) }
            .expect("Failed to reset fence");

        let (image_index, _) = self
            .boilerplate
            .swapchain
            .acquire_next_image(self.present_semaphore)
            .expect("Failed to acquire next image");

        self.boilerplate.command_manager.begin_main_command_buffer();

        let flash = 0.0;

        let clear_values = [
            ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, flash, 1.0],
                },
            },
            ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];

        let render_pass_begin_info = vk::RenderPassBeginInfo::builder()
            .render_pass(self.render_pass)
            .render_area(Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.boilerplate.swapchain.extent,
            })
            .framebuffer(self.framebuffers[image_index as usize])
            .clear_values(&clear_values)
            .build();

        self.boilerplate
            .command_manager
            .begin_render_pass(&render_pass_begin_info);

        self.boilerplate
            .command_manager
            .bind_pipeline(&self.mesh_pipeline);

        let cam_pos = glm::vec3(0.0, 0.0, -2.0);
        let view_mat = glm::translate(&glm::Mat4::identity(), &cam_pos);

        let mut proj_mat = glm::perspective(
            1600.0 / 900.0,
            glm::radians(&glm::vec1(70.0))[0],
            0.1,
            200.0,
        );

        proj_mat[(1, 1)] *= -1.0;

        let view_proj_mat = proj_mat * view_mat;

        for (_, mesh) in &mut self.meshes.iter_mut() {
            let offset = 0;
            self.boilerplate.command_manager.bind_vertex_buffers(
                0,
                &[mesh
                    .as_ref()
                    .borrow_mut()
                    .vertex_buffer
                    .as_ref()
                    .unwrap()
                    .buffer],
                &[offset],
            );

            let model_mat = glm::rotate(
                &glm::Mat4::identity(),
                self.framenumber as f32 / 100.0,
                &glm::vec3(0.0, 1.0, 0.0),
            );

            let mvp: nalgebra::Matrix<
                f32,
                nalgebra::Const<4>,
                nalgebra::Const<4>,
                nalgebra::ArrayStorage<f32, 4, 4>,
            > = view_proj_mat * model_mat;

            let push_constants = MeshPushConstants {
                render_matrix: mvp,
                data: glm::vec4(0.0, 0.0, 0.0, 0.0),
            };

            self.boilerplate
                .command_manager
                .push_constants(self.mesh_pipeline.pipeline_layout, push_constants);

            self.boilerplate
                .command_manager
                .draw(mesh.as_ref().borrow_mut().vertex_count, 1, 0, 0);
        }

        self.boilerplate.command_manager.end_render_pass();

        self.boilerplate
            .command_manager
            .end_main_command_buffer()
            .unwrap();

        self.boilerplate.command_manager.submit_main_command_buffer(
            &[self.present_semaphore],
            &[self.present_semaphore],
            self.fence,
        );

        self.boilerplate.swapchain.present(
            &self.boilerplate.queue,
            image_index,
            &[self.present_semaphore],
        );

        self.framenumber += 1;
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        trace!("Cleaning: Renderer");

        unsafe {
            self.boilerplate
                .device
                .wait_for_fences(&[self.fence], true, 1000000000)
                .expect("Failed to wait for fence");

            for mesh in self.meshes.iter_mut() {
                mesh.1
                    .as_ref()
                    .borrow_mut()
                    .free(&mut self.boilerplate.allocator)
            }

            self.boilerplate
                .device
                .destroy_semaphore(self.render_semaphore, None);
            self.boilerplate
                .device
                .destroy_semaphore(self.present_semaphore, None);
            self.boilerplate.device.destroy_fence(self.fence, None);

            ManuallyDrop::drop(&mut self.mesh_pipeline);

            for framebuffer in &self.framebuffers {
                self.boilerplate
                    .device
                    .destroy_framebuffer(*framebuffer, None);
            }
            self.boilerplate
                .device
                .destroy_render_pass(self.render_pass, None);
        }
    }
}
