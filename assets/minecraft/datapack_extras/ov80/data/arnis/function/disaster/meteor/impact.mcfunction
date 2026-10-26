execute unless entity @s[tag=arnis.ds.big] run summon fireball ~ ~1 ~ {ExplosionPower:2b,Motion:[0.0d,-1.0d,0.0d],acceleration_power:0.1d,Tags:["arnis.ds.ent"]}
execute if entity @s[tag=arnis.ds.big] run summon fireball ~ ~1 ~ {ExplosionPower:4b,Motion:[0.0d,-1.0d,0.0d],acceleration_power:0.1d,Tags:["arnis.ds.ent"]}
tag @s remove arnis.ds.impact
tag @s add arnis.ds.crater
scoreboard players set @s arnis.ds.t 4
