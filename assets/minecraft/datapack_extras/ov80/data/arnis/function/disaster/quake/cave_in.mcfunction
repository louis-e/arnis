# The roof gives way.
execute if score #pass arnis.ds matches ..1 run summon fireball ~ ~1 ~ {ExplosionPower:3b,Motion:[0.0d,-1.0d,0.0d],acceleration_power:0.1d,Tags:["arnis.ds.ent"]}
execute if score #pass arnis.ds matches 2.. run summon fireball ~ ~1 ~ {ExplosionPower:4b,Motion:[0.0d,-1.0d,0.0d],acceleration_power:0.1d,Tags:["arnis.ds.ent"]}
function arnis:disaster/quake/debris
particle minecraft:campfire_cosy_smoke ~ ~ ~ 3 2 3 0.02 10
playsound minecraft:block.stone.break block @a[distance=..48] ~ ~ ~ 2 0.5
