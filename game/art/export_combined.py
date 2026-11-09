import bpy, os, math
import numpy as np
from mathutils import Vector

ART = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(ART, "character_combined.glb")
QUINN = os.path.join(ART, "quinn_anim.glb")
MESHY = os.path.join(ART, "teal_vanguard.glb")

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=QUINN)
arm = bpy.data.objects.get('Unreal Engine 5 (Quinn)')
assert arm and arm.type == 'ARMATURE'
ico = bpy.data.objects.get('Icosphere')
if ico:
    bpy.data.objects.remove(ico, do_unlink=True)
for b in arm.data.bones:
    if b.name.startswith('ik_') or b.name in ('root', 'interaction', 'center_of_mass'):
        b.use_deform = False

bpy.ops.import_scene.gltf(filepath=MESHY)
mesh = next(o for o in bpy.data.objects if o.type == 'MESH')
print("MESH", mesh.name, "dims", tuple(round(v,4) for v in mesh.dimensions))

DEFORM = [
    'pelvis','spine_01','spine_02','spine_03','spine_04','spine_05',
    'neck_01','neck_02','head',
    'clavicle_l','upperarm_l','lowerarm_l','hand_l',
    'clavicle_r','upperarm_r','lowerarm_r','hand_r',
    'thigh_l','calf_l','foot_l','ball_l',
    'thigh_r','calf_r','foot_r','ball_r',
]
DEFORM = [n for n in DEFORM if n in arm.data.bones]
A = np.zeros((len(DEFORM), 3)); B = np.zeros((len(DEFORM), 3))
for i, name in enumerate(DEFORM):
    b = arm.data.bones[name]
    A[i] = np.array((arm.matrix_world @ b.head_local))
    B[i] = np.array((arm.matrix_world @ b.tail_local))
N = len(mesh.data.vertices)
P = np.empty((N, 3))
for i, v in enumerate(mesh.data.vertices):
    P[i] = np.array((mesh.matrix_world @ v.co))
D = np.empty((N, len(DEFORM)))
for j in range(len(DEFORM)):
    ab = B[j] - A[j]; denom = float(np.dot(ab, ab))
    if denom < 1e-9:
        D[:, j] = np.linalg.norm(P - A[j], axis=1)
    else:
        t = np.clip(((P - A[j]) @ ab) / denom, 0.0, 1.0)
        D[:, j] = np.linalg.norm(P - (A[j] + t[:, None] * ab), axis=1)
D = np.maximum(D, 1e-4)
W = 1.0 / D ** 4
for j, name in enumerate(DEFORM):
    if name.endswith('_l'):
        W[P[:, 0] < -0.035, j] = 0.0
    elif name.endswith('_r'):
        W[P[:, 0] > 0.035, j] = 0.0
nearest = D.min(axis=1, keepdims=True)
W[D > nearest * 2.1 + 0.02] = 0.0
for k in (3,):
    cut = np.partition(W, -k, axis=1)[:, -k][:, None]
    W[W < cut] = 0.0
W /= W.sum(axis=1, keepdims=True)
edges = np.array([e.vertices[:] for e in mesh.data.edges], dtype=np.int64)
neighbors = [[] for _ in range(N)]
for a, b in edges:
    neighbors[a].append(b); neighbors[b].append(a)
for _ in range(3):
    W2 = W.copy()
    for i in range(N):
        if neighbors[i]:
            W2[i] = 0.55 * W[i] + 0.45 * W[neighbors[i]].mean(axis=0)
    W = W2
    W[W < 0.02] = 0.0
    # glTF supports four influences per vertex.
    cut = np.partition(W, -4, axis=1)[:, -4][:, None]
    W[W < cut] = 0.0
    W /= W.sum(axis=1, keepdims=True)
groups = {name: mesh.vertex_groups.new(name=name) for name in DEFORM}
for i in range(N):
    for j in np.nonzero(W[i])[0]:
        groups[DEFORM[j]].add([i], float(W[i, j]), 'REPLACE')
print("UNWEIGHTED", sum(1 for v in mesh.data.vertices if not v.groups))

bpy.ops.object.select_all(action='DESELECT')
mesh.select_set(True); arm.select_set(True)
bpy.context.view_layer.objects.active = arm
bpy.ops.object.parent_set(type='ARMATURE_NAME')

arm.data.pose_position = 'POSE'
ad = arm.animation_data or arm.animation_data_create()
ad.action = None
bpy.context.scene.frame_set(0)
bpy.ops.object.select_all(action='DESELECT')
mesh.select_set(True); arm.select_set(True)
bpy.context.view_layer.objects.active = arm

bpy.ops.export_scene.gltf(
    filepath=OUT,
    export_format='GLB',
    use_selection=True,
    export_apply=False,
    export_yup=True,
    export_skins=True,
    export_animations=True,
    export_animation_mode='ACTIONS',
    export_bake_animation=True,
    export_force_sampling=True,
    export_frame_range=False,
    export_anim_single_armature=True,
    export_reset_pose_bones=True,
    export_optimize_animation_size=False,
    export_materials='EXPORT',
    export_cameras=False,
    export_lights=False,
)
print("EXPORTED", OUT)
