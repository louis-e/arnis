# The way the wind blows for the whole event, as a facing and as a direction to move in.
execute store result score #r arnis.ds run random value 0..7
execute if score #r arnis.ds matches 0 run data modify storage arnis:ds wind merge value {yaw:0.0f,vx:0.0d,vz:1.0d}
execute if score #r arnis.ds matches 1 run data modify storage arnis:ds wind merge value {yaw:45.0f,vx:-0.707d,vz:0.707d}
execute if score #r arnis.ds matches 2 run data modify storage arnis:ds wind merge value {yaw:90.0f,vx:-1.0d,vz:0.0d}
execute if score #r arnis.ds matches 3 run data modify storage arnis:ds wind merge value {yaw:135.0f,vx:-0.707d,vz:-0.707d}
execute if score #r arnis.ds matches 4 run data modify storage arnis:ds wind merge value {yaw:180.0f,vx:0.0d,vz:-1.0d}
execute if score #r arnis.ds matches 5 run data modify storage arnis:ds wind merge value {yaw:225.0f,vx:0.707d,vz:-0.707d}
execute if score #r arnis.ds matches 6 run data modify storage arnis:ds wind merge value {yaw:270.0f,vx:1.0d,vz:0.0d}
execute if score #r arnis.ds matches 7 run data modify storage arnis:ds wind merge value {yaw:315.0f,vx:0.707d,vz:0.707d}
scoreboard players set #wind arnis.ds 1
scoreboard players set #gust arnis.ds 60
